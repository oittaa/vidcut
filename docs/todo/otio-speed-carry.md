# Carry speed changes through vidcut

**When:** next, after [compound clips](otio-compound-clips.md). The **Shared** part of [clip effects](otio-carry-clip-effects.md) applies here. Playing them at their speed is [speed changes](speed-changes.md).

Resolve writes a constant speed as OTIO's standard `LinearTimeWarp` (`time_scalar` 2.0, 0.5, and −1.0 for reverse). Today an import plays such a clip at normal speed for its length: other frames than in Resolve, and the speed is lost on the way back.

- Keep the speed on the clip and write it back. The clip shows it (`200%`) and plays at normal speed, as now.
- Lock what needs speed math: trim, split, slip, slide and J/L rolls refuse the clip (cutting 10 frames off a clip at 200% removes 20 frames of its media). Moving and deleting work.
- Speed ramps (Resolve 21's retime curves) weren't tried; however Resolve writes them, they are carried and locked the same way.
