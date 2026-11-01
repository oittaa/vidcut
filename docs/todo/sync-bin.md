# Sync Bin

**When:** not decided.

The Cut page names the Sync Bin as later work ([spec §1](../cut-page-spec.md#1-scope)). vidcut has no synced angles and no Sync Bin.

**Why:**

- Multicam cutting is a standing request among open-source editors.
- Several cameras of one moment need a quick way to pick the angle.
- The Cut page gathers those angles in a Sync Bin.

**In the code:**

- `MediaInfo.timecode` stores a file's start timecode when the file has one.
- `VideoInfo.first_pts` and `AudioInfo.first_pts` store the first presentation time.
- `Source::advance` shows the frame whose presentation time has reached the target.
- `VideoInfo.vfr` records a variable frame rate. The reader still follows presentation time.
- Waveforms draw on clips.
- No command aligns two pool items. No command switches angles.

**To do:**

- Read the Sync Bin in the Cut page chapters, then measure it in Resolve.
- Record how angles sync, and how a cut chooses an angle.
- Sync by start timecode when both files have one.
- Otherwise sync by the waveform.
- A marker is the sync point when the user sets one.
- An angle switch replaces the picture and keeps the sound the user chose.
- The switch is one timeline edit.
- The CLI, IPC and MCP perform the same action as the GUI.
