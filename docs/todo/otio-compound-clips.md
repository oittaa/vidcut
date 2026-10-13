# Compound clips and nested timelines

**When:** next. Carried as [clip effects](../deviations.md#x9-opentimelineio-interchange) are (deviation X9): kept as data, shown, played as today, written back unchanged; Resolve rebuilds what it wrote and drops other programs' metadata.

Resolve writes a compound clip or a nested timeline as a nested OTIO `Stack` with all of its own tracks and clips (`Resolve_OTIO` says `Compound Clip` or `Timeline Clip`, with an id), and rebuilds the compound clip from it. Today vidcut leaves the whole item out: a gap, its picture and sound gone.

- **Carry, by default:** one block clip that can be moved, trimmed at its ends and deleted, written back as it came, so Resolve gets its compound back. It plays as a gap at first; playing its contents (ordinary clips, expanded while the render plan is made) can follow.
- **Flatten, when asked:** an import option, and a command for a carried block later (Resolve's Decompose in Place), put its clips on vidcut's tracks at their times, trimmed to the part used: editable, but no longer a compound. Contents on several tracks need tracks above; a nested timeline used twice is copied twice; effects on the compound itself go onto each clip or are dropped, and the result says which.
