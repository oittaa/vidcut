# Sync Bin

**When:** not decided.

The Cut page names the Sync Bin as later work ([spec §1](../cut-page-spec.md#1-scope)). vidcut has no synced angles and no Sync Bin.

**Why:**

- The Cut page gathers several angles of one moment in a Sync Bin.
- vidcut stores no synced group.

**In the code:**

- `MediaInfo.timecode` stores a file's start timecode when the file has one.
- `VideoInfo.first_pts` and `AudioInfo.first_pts` store the first presentation time.
- `Source::advance` shows the frame whose presentation time has reached the target.
- `VideoInfo.vfr` records a variable frame rate. The reader still follows presentation time.
- Waveforms draw on clips.
- `multicam.layout` works out where angles lie, by timecode or marks. It stores nothing.
- `multicam.create` stores a synced group, a pool item.
- No command switches angles.

**To do:**

- Open a group in the Sync Clips window (p. 708).
- For the Edit page stage, open a multicam clip as a timeline (p. 1040).
- Read the Sync Bin in the Cut page chapters, then measure it in Resolve.
- Record how angles sync, and how a cut chooses an angle.
- Measure sync by sound with files from real cameras, each with its own microphone. Then check `SOUND_MATCH_SCORE`.
- A marker is the sync point when the user sets one.
- An angle switch replaces the picture and keeps the sound the user chose.
- The switch is one timeline edit.
- The CLI, IPC and MCP perform the same action as the GUI.
- Measure the cases that Source Overwrite refuses ([spec §7.2](../cut-page-spec.md#72-edit-functions)).
  - Do audio-only files and other frame rates first, because sound recorders make them.
- Measure how Source Overwrite treats hidden tracks, titles and compound clips.
- Measure which file tags give the camera name, with files from real cameras. Then `media.add` reads the name.
