# Carry Resolve's clip effects through vidcut

**When:** next (roadmap use case 1). The first of five items that carry what vidcut doesn't edit; the others are [transitions](otio-carry-transitions.md), [generators and titles](otio-carry-generators.md), [compound clips](otio-compound-clips.md) and [speed changes](otio-speed-carry.md).

**Shared:** an import leaves out, with a warning each, what vidcut can't edit, so a timeline cut in vidcut, finished in Resolve and brought back to recut loses what was added in Resolve. Instead, keep it as data: on the clip or the edit, shown (a badge or a placeholder), rendered as today (cuts, normal speed, gaps), and written back unchanged on export. Editing it stays out of scope (spec §1). Resolve rebuilds all of these from its own OTIO (checked with Studio 21.1.1), so carrying them unchanged brings them back. Resolve drops other programs' metadata, so nothing can rely on `metadata.vidcut` coming back.

**Clip effects:** Resolve writes every clip's whole effect stack in `metadata.Resolve_OTIO` (Transform, Cropping, Composite, Video Faders, Dynamic Zoom, Fairlight Clip Volume and Fades, Fairlight Clip Pan, …), each parameter with its value, default and keyframes. Positions are normalized to the frame: a 100-pixel pan in 1920 is 0.052.

- Keep a clip's effect list as it came, and write it back. Clip volume stays clip gain, as now.
- A split gives both halves the effects, but a fade-in stays with the first half and a fade-out with the last. Keyframes count from the clip's start, so trimming its head shifts them, or a clip with keyframes is locked against trims.
- The clip shows a badge naming them (`Transform, Cropping`); the warnings say they were kept, not shown.
