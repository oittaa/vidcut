# Adjustment layers

**When:** not decided.

Effects run on one clip's own picture (`fs_blur`), and then the tracks mix.

**Why:**

- One blur or one look on a stack of clips is a usual request.
- Each clip in the stack needs its own copy of the effect.
- The Cut page has no adjustment clip. Record a deviation when this is built.

**To do:**

- An adjustment clip carries effects and has no media.
- The viewer and the export mix the tracks below that clip, then run its effects.
- The effects read the 16-bit float mix the draw already keeps.
- Measure an OpenTimelineIO shape that Resolve reads back, before vidcut writes one.
- Gaussian Blur is the first effect on that mix.
- The CLI, IPC and MCP perform the same action as the GUI.
