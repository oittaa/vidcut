# Destination controls

**When:** not decided.

The Edit page has a source destination control for each track of the source, in the Source Track Selector beside the track headers (pp. 896–898).
vidcut has no such control. Its edits use the target track ([D3](../spec.md#12-decisions-on-gaps-and-contradictions)) and Video Only and Audio Only.

**Why:** the user picks the timeline track of each source track. With Decompose Compound Clips On Edit, this works for each track inside a compound clip (p. 1055).

**What Resolve has:**

- By default, the controls are on V1 and A1 (p. 896).
- A drag moves a control to another track (p. 896). Keys also move and set the controls (p. 897).
- A click on a control turns it off. The edit then takes only the picture or only the sound (p. 898).
- Edit > Edit Options > Automatically Create Tracks on Edit is on by default. It makes new tracks for a source with more audio tracks than the timeline (p. 896).
  - With it off, the edit takes only the source tracks that the controls show (p. 896).
- With Decompose Compound Clips On Edit, a compound clip or a timeline in the source shows one control for each of its tracks (p. 1055).

**Measured:** the controls of a compound clip, in [§8.18](../spec.md#818-compound-clips-pp-10521054-1126-measured-in-resolve-studio-2120).

**In the code:**

- `SourceParams::inner_tracks` gives the timeline track of each track inside a compound clip. `null` turns a track off. On a picture track, it leaves out only the picture. The CLI, IPC and MCP have it, and the GUI does not.
- A clip on a video track carries its sound (`ClipAudio`). A control that sends the sound to a track apart from the picture needs a sound clip apart from the picture clip.

**To do:**

- Measure the controls for a clip of a file: the default places, the keys and a swap.
- Measure a source with more audio tracks than the timeline.
- Show a control for each source track beside the track headers.
- Send the edits of [§7.2](../spec.md#72-edit-functions) through the controls. `inner_tracks` stays their scripted form for a compound clip.

**Open questions:**

- The target track and Video Only and Audio Only do a part of this work. The measurements decide how they and the controls work together.
