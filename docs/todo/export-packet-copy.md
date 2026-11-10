# Export that copies the source packets

**When:** not decided.

An export decodes and encodes each frame. A packet copy writes the packets of the source where the timeline does not change the picture. Other editors name this a smart cut or a lossless cut.

**Why:**

- The quick edit of the [roadmap](../roadmap.md#use-cases) cuts the silent parts out of a long recording and adds an intro. Most of its frames do not change.
- A copied packet needs no decode and no encode. The export of such an edit then needs little more time than a file copy.
- This helps a computer with no GPU encoder most.
- A copied packet keeps the quality of the source. A new lossy encode loses some of it.

**In the code:**

- The export thread draws each frame of the plan (`VideoFrames::next_frame`). The writer encodes each one ([export](../design/export.md#encoding-one-output)).
- No path copies a packet of a source into an output.
- The plan names the pieces with one source: a `VideoPiece::Source` holds the source, its `pace` and its `Placed` list.
- The export settings choose the codec, the size and the quality ([export](../design/export.md#choosing-an-encoder)).

**To do:**

- Measure Bypass Re-Encode When Possible of Resolve's Deliver page.
  - Measure which clips it copies.
  - Measure what it does at a cut inside a group of pictures.
- The Cut page Quick Export has no such control. Record a deviation if this item is built.
- Decide the control. The user asks for the copy, because the copied frames do not get the quality of the export settings (AGENTS.md rule 10).
- The rule for a piece that the export can copy:
  - It is one source at speed 1, with no effect, no placement, no title, no subtitle picture and no LUT.
  - The codec, the size, the frame rate, the pixel format and the colors of the source are those of the output.
  - Its first frame is a keyframe of the source, and none of its frames needs a frame outside it.
- The frames between a cut and the next keyframe get an encode. The decoder must take those packets and the copied packets as one stream.
- The sound gets an encode in each case. Thus the mix, the fades and the sound tools stay.
- `export.plan` tells how many frames the export copies. The report of the export tells the same.
- Check the result in several players: the picture at each join, the seek, and the times of a source with a variable frame rate.
- The CLI, IPC and MCP perform the same action as the GUI.
