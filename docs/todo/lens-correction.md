# Lens Correction

**When:** not decided.

The Cut page viewer Tools correct lens distortion. vidcut has none.

**Why:**

- Analyze reads the frame at the playhead and sets a distortion value.
- Distortion warps the picture by hand.
- The same values are in the Video Inspector.
- The Cut page Inspector has keyframes for that group ([keyframes.md](keyframes.md)).

**In the code:**

- The design lists Lens Correction among the stages vidcut does not have.
- The keyframe groups vidcut edits do not include it.

**To do:**

- Measure Analyze, the distortion range, and what an OpenTimelineIO file keeps.
- One request analyzes the selected clip.
- One request sets the distortion.
- The viewer and the export play that warp.
- The CLI, IPC and MCP perform the same action as the GUI.
