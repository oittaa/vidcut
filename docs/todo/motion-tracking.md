# Motion tracking

**When:** not decided.

A track is the path of a point or an area of the picture through the frames of a clip. vidcut makes no track.

**Why:**

- A blur over a face or a number plate must follow it.
- A title or a picture that points at a thing must follow it.
- Without a track, the user sets a keyframe by hand at each move.

**In the code:**

- Transform, Crop and the values of the effects have keyframes ([keyframes.md](keyframes.md)).
- A Glow stores Follow FX Tracker (`atmosphere_tracked`). The value does not change the picture ([spec §8.16](../cut-page-spec.md#816-effects-pp-765-3526-3536)).
- No request analyzes the motion of a picture. [Stabilization](stabilization.md) needs such an analysis too.
- A [subject mask](subject-masks.md) is a matte that a track can move.

**To do:**

- Measure where Resolve makes a track that the Cut page plays: the FX Tracker, and the Tracker of the Color page.
- Record a deviation if the Cut page makes no track and this item is built.
- Measure what an OpenTimelineIO file keeps of a track.
- Decide what a track gives. The simplest result is keyframes of values that already have them.
- One request tracks a point or a box of the selected clip from the playhead, forward or backward.
- The request tells the frame where it loses the subject. The user starts again from that frame.
- A blur with an area: an effect needs a box or an ellipse before a track can move it.
- The viewer and the export play the same result.
- The CLI, IPC and MCP perform the same action as the GUI.
