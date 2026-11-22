# Destination controls

**When:** not decided.

The Edit page has a source destination control for each track of the source, in the Source Track Selector beside the track headers (pp. 896–898).
vidcut has no such control. Its edits use the target track ([D3](../spec.md#12-decisions-on-gaps-and-contradictions)) and Video Only and Audio Only.

**Why:** the user picks the timeline track of each source track. With Decompose Compound Clips On Edit, this works for each track inside a compound clip (p. 1055).

**What Resolve has:**

- By default, the controls are on V1 and A1 (p. 896).
- A drag moves a control to another track (p. 896). Keys also move and set the controls (p. 897).
- A click on a control turns it off. The edit then takes only the picture or only the sound (p. 898).
- Audio Track Creation While Editing makes new audio tracks for a source with more audio tracks than the timeline (p. 896).
- With Decompose Compound Clips On Edit, a compound clip or a timeline in the source shows one control for each of its tracks (p. 1055).

**Measured in Resolve Studio 21.2** (Edit page, Decompose Compound Clips On Edit on):

- The compound clip holds clips on V1 and V2. The V1 clip has sound.

- The controls V1, V2 and A1 show at the timeline tracks of the same number. With the option off, only V1 and A1 show.
- A drag of V2 to Video 3 puts the clip of inner V2 on V3.
- A drop of V2 on the track of V1 swaps the two controls.
- With V1 off, V1 of the timeline does not change. The sound of the clip on inner V1 still goes on A1.
- With A1 off, the pictures go in, and A1 of the timeline does not change.

**In the code:**

- `SourceParams::inner_tracks` gives the timeline track of each track inside a compound clip. `null` turns a track off. The CLI, IPC and MCP have it, and the GUI does not.
- A clip on a video track carries its sound (`ClipAudio`). A control that sends the sound to a track apart from the picture needs a sound clip apart from the picture clip.

**To do:**

- Measure the controls for a clip of a file: the default places, the keys and a swap.
- Measure a source with more audio tracks than the timeline.
- Show a control for each source track beside the track headers.
- Send the edits of [§7.2](../spec.md#72-edit-functions) through the controls. `inner_tracks` stays their scripted form for a compound clip.

**Open questions:**

- The target track and Video Only and Audio Only do a part of this work. The measurements decide how they and the controls work together.
