# Stats for nerds

**When:** after VideoToolbox decoding.

A panel over the viewer, opened from its right-click menu, showing what the player does:
- the viewer's size, frames shown, late and skipped
- the picture's size and rate, and the timeline's
- the codecs, and the decoder used: FFmpeg's and on which GPU, or the CPU and why
- the picture's layout and colors, where it is, and how it got on screen
- pictures kept ahead and behind, against the budgets
- the sound device, rate and latency, and the renderer's backend and adapter

A request toggles it, and `player.state` returns the same facts.
