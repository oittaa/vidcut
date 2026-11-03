# Sync Bin

**When:** not decided.

The Sync Bin tab, the synced group and Multi Source are in vidcut ([spec §7.4](../cut-page-spec.md#74-sync-bin-pp-708710-measured-in-resolve-studio-2112)). This file lists what is still open.

**Why:**

- The Cut page reopens a group in the Sync Clips window.
- The Edit page opens a multicam clip as a timeline, and a cut chooses an angle.

**In the code:**

- `MediaInfo.timecode` stores a file's start timecode when the file has one.
- `VideoInfo.first_pts` and `AudioInfo.first_pts` store the first presentation time.
- `Source::advance` shows the frame whose presentation time has reached the target.
- `VideoInfo.vfr` records a variable frame rate. The reader still follows presentation time.
- Waveforms draw on clips.
- `multicam.layout` works out where angles lie, by timecode or marks. It stores nothing.
- `multicam.create` stores a synced group, a pool item.
- `edit.source_overwrite` with `group` lines up by a synced group.
- `multicam.rows` and `multicam.pick` give the rows of the Sync Bin and choose a camera.
- No command switches angles.

**To do:**

- Open a group in the Sync Clips window (p. 708).
- For the Edit page stage, open a multicam clip as a timeline (p. 1040).
- Measure the Sync Bin parts that are not measured:
  - the number keys and Escape;
  - the sound of the grid with Video Only off and on (p. 708);
  - a grid of more than 4 cameras;
  - the Out mark by hand in the view of one camera;
  - a drag on the rows in the view of one camera;
  - rows for files with no tag and no saved group, with no camera name, and with no picture;
  - a timeline whose clips disagree on the time of a file.
- Show the Sync Bin grid in the Cinema Viewer with P (p. 709).
- Record how a cut chooses an angle.
- Measure sync by sound with files from real cameras, each with its own microphone. Then check `SOUND_MATCH_SCORE`.
- A marker is the sync point when the user sets one.
- An angle switch replaces the picture and keeps the sound the user chose.
- The switch is one timeline edit.
- The CLI, IPC and MCP perform the same action as the GUI.
- Measure the cases that Source Overwrite refuses ([spec §7.2](../cut-page-spec.md#72-edit-functions)).
  - Do audio-only files and other frame rates first, because sound recorders make them.
- Measure how Source Overwrite treats hidden tracks, titles and compound clips.
- Measure which file tags give the camera name, with files from real cameras. Then `media.add` reads the name.
