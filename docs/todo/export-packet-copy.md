# Export that copies the source packets

**When:** not decided.

An export decodes and encodes each frame. A packet copy writes the packets of the source where the timeline does not change the picture. Other editors name this a smart cut or a lossless cut.

**Why:**

- The quick edit of the [roadmap](../roadmap.md#use-cases) cuts the silent parts out of a long recording and adds an intro. Most of its frames do not change.
- A copied packet needs no decode and no encode. The export of such an edit then needs little more time than a file copy.
- This helps a computer with no GPU encoder most.
- A copied packet keeps the quality of the source. A new lossy encode loses some of it.

**What the manual says:**

- The Video panel of the Deliver page has Bypass re-encode when possible, under Advanced Controls (pp. 4115, 4119–4120).
- With it on, Resolve copies the video of a timeline clip from the source file into the output.
  - It does this when the Format, the Codec and the Type of the output match the source.
- The copy works only for a clip with no added effect.
  - A grade, a Resolve FX, a composite, a resize, a stabilization or a Fusion effect makes Resolve encode the whole clip.
  - A change of the output sizing of the timeline does the same.
- A transition gets an encode only for its own length.
- A new sound mix on a copied picture needs no new encode of the picture.
- The list of the formats that take the copy is on the support page of Blackmagic Design, not in the manual.
- A search of the Quick Export chapter (pp. 788–791) finds no such control.

**In the code:**

- The export thread draws each frame of the plan (`VideoFrames::next_frame`). The writer encodes each one ([export](../design/export.md#encoding-one-output)).
- No path copies a packet of a source into an output.
- The plan names the pieces with one source: a `VideoPiece::Source` holds the source, its `pace` and its `Placed` list.
- The export settings choose the codec, the size and the quality ([export](../design/export.md#choosing-an-encoder)).

**To do:**

- Measure the lines of the manual above in Resolve. The manual and the application can differ.
- Measure what Resolve does at a cut inside a group of pictures. The manual does not tell.
- Record a deviation if Quick Export gets this control.
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
