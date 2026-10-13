# Carry transitions through vidcut

**When:** next. Carried as [clip effects](../deviations.md#x9-opentimelineio-interchange) are (deviation X9): kept as data, shown, played as today, written back unchanged; Resolve rebuilds what it wrote and drops other programs' metadata.

Resolve writes a video dissolve as OTIO's standard `Transition` (`SMPTE_Dissolve`, with in and out offsets) and an audio crossfade as a `Custom_Transition` named after it ("Cross Fade +3 dB"). Blender's coming OTIO support writes its crossfades and wipes the same way.

- Keep a transition on its edit point and write it back, while the two clips stay neighbors at that cut with media enough for its offsets. Otherwise it is dropped, and the result says so.
- The preview and exports show a cut, as today; the timeline marks the edit point.
