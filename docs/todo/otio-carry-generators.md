# Carry generators and titles through vidcut

**When:** next. Carried as [clip effects](../deviations.md#x9-opentimelineio-interchange) are (deviation X9): kept as data, shown, played as today, written back unchanged; Resolve rebuilds what it wrote and drops other programs' metadata.

Resolve writes generators (Solid Color) and standard titles as OTIO `GeneratorReference`s, their settings in `Resolve_OTIO` (a title's text among them). A Fusion title (Text+) comes out as a `MissingReference` with nothing in it: no program can do better than a gap for it.

- Keep each as a placeholder clip: a named, colored block that plays as a gap, can be moved, trimmed and deleted, and is written back as it came.
