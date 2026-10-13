# Speed changes

**When:** next. Clips at a speed play and edit (spec D48).

- **Setting:** Change Clip Speed (Resolve's Cut page has it) in the GUI, CLI, IPC and MCP, and speed changes into scope (spec §1). Resolve keeps a clip's length without ripple and its source with ripple (99 frames at 50% become 198).
- **Interchange:** OTIO's `LinearTimeWarp` written for speeds set in vidcut.
- **Analyses:** Ripple Delete Silence and Detect Scene Cuts look at clips at another speed, their source mapped to the timeline through it (`analysis::Span`).
- **Compound clips:** one with a speed change plays the clips it holds at it (`render::expanded`), backward in reverse order, and decomposes.
