# Compound clips made in vidcut

**When:** not decided.

vidcut plays a compound clip from another editor. `edit.decompose` opens one. No request builds one from timeline clips.

**Why:**

- Users ask for nested timelines so a scene, a title card or a stack stays one clip.
- Flat timelines are a standing complaint about open-source editors.
- An OpenTimelineIO file from Resolve keeps compound clips ([deviation X9](../deviations.md#x9-opentimelineio-interchange)).
- vidcut already plays that shape, so a nest made here can leave and come back.

**In the code:**

- `timeline.import` with mode `compound` adds one compound clip after the last clip.
- `Compound` stores the nested stack and the tracks that play (`tracks`).
- `render::flatten` plays those tracks inside the host clip.
- `edit.decompose` puts the inner clips back on the timeline.
- Opacity and effects of a compound clip are a separate item ([compound-opacity.md](compound-opacity.md)).

**To do:**

- Measure a compound clip that Resolve made, in the OpenTimelineIO file it writes.
- Add one request that nests the selected clips into a compound clip of that shape.
- The picture and the sound stay linked, as they do on an imported compound clip.
- Playback and export use the plan an imported compound clip already uses.
- Decompose restores the inner clips and their edits.
- The CLI, IPC and MCP perform the same action as the GUI.
