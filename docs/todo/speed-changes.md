# Speed changes

**When:** next. Clips at a speed play (spec D48); edits that would need speed math are refused (deviation X9).

- **Editing:** every edit at a clip's edge converts timeline frames to media frames through its speed: trim, roll, ripple, slip, slide, split, J/L rolls. Splitting a clip at 200% at its 25th frame splits its media at frame 50. Media limits count through it too.
- **Setting:** Change Clip Speed (Resolve's Cut page has it) in the GUI, CLI, IPC and MCP, and speed changes into scope (spec §1).
- **Interchange:** OTIO's `LinearTimeWarp` both ways as the clip's own speed, replacing the carrying.
- **Compound clips:** one with a speed change plays the clips it holds at it (`render::expanded`), backward in reverse order.
