# Adjustment layers

**When:** not decided.

Effects run on one clip's own picture (`fs_blur`), and then the tracks mix.

**Why:**

- Each clip in a stack needs its own copy of an effect.
- The Cut page has no adjustment clip. Record a deviation when this is built.

**To do:**

- An adjustment clip carries effects and has no media.
- The viewer and the export mix the tracks below that clip, then run its effects.
- The effects read the mix in `Wide` (`Rgba16Float`).
- Measure an OpenTimelineIO shape that Resolve reads back, before vidcut writes one.
- Gaussian Blur is the first effect on that mix.
- The CLI, IPC and MCP perform the same action as the GUI.
