# Speed changes

**When:** later. Until then they are [carried and locked](otio-speed-carry.md). Out of scope today (spec §1), so this needs a deviation.

Playing and exporting clips at a constant speed (2, ½, −1 for reverse, 0 for a freeze), and setting it (Resolve's Cut page has Change Clip Speed):

- **Editing:** every edit at a clip's edge converts timeline frames to media frames through its speed: trim, roll, ripple, slip, slide, split, J/L rolls. Splitting a clip at 200% at its 25th frame splits its media at frame 50.
- **Picture:** output frame `t` shows media frame `in + (t − start) × speed`, by the sampling rule (export.md §2): every other frame at 200%, each frame twice at 50% (Resolve's frame blending and optical flow stay out). Reverse decodes backward, as the viewer already does for J; exports would too. The same in the preview, exports, frame renders, storyboards and proxies.
- **Sound:** pitch kept, as Resolve's pitch correction does by default: the player's time-stretch (`player/stretch.rs`) and reversed sound, in the export's mix too.
- **Interchange:** OTIO's `LinearTimeWarp` both ways, replacing the carrying.
