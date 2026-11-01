# Subject masks

**When:** not decided.

vidcut has no matte of a person, and no matte of a subject the user points at.

**Why:**

- The Cut page has no matte of a person. Record a deviation if this item is built.
- A composite mask mode takes its matte from the clip below ([composite-masks.md](composite-masks.md)).
- A subject mask is a matte of the clip itself.

**In the code:**

- A source with an alpha channel composites over the tracks below it.
- The composite is straight alpha.
- Voice Isolation, Super Scale and Retime Process run local models.
- Each missing model asks for a download and checks a SHA-256 (`isolate`, `models`).

**To do:**

- One request makes a matte for the selected clip.
- The subject is a person, or a point on the picture.
- The matte is that clip's alpha in the viewer and in the export.
- The model runs on this computer. Nothing is uploaded.
- The first model finds a person.
- Check its mattes on pictures the model did not learn from.
- A subject at a point is a later model.
- Keyframes of the matte, and a drawn curve, wait until the still matte is right.
- The CLI, IPC and MCP perform the same action as the GUI.
