# Export that copies the source packets

**When:** not decided.

An export decodes and encodes each frame. A packet copy writes the packets of the source where the timeline does not change the picture.

**Why:**

- The quick edit of the [roadmap](../roadmap.md#use-cases) puts an intro and an outro around a recording, with its silent parts cut out.
- The roadmap asks vidcut to run well with no GPU on a modest CPU.
- The manual says that the copy keeps the quality and makes the output much faster (p. 4120).

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
- Write the rule for a piece that the export can copy. Start from the rule of the manual above.
- Decide what the export does between a cut and the next keyframe of the source.
- `export.plan` tells how many frames the export copies. The report of the export tells the same.
- The CLI, IPC and MCP perform the same action as the GUI.
