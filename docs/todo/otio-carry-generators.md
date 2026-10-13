# Carry generators and titles through vidcut

**When:** next, after [transitions](otio-carry-transitions.md). The **Shared** part of [clip effects](otio-carry-clip-effects.md) applies here.

Resolve writes generators (Solid Color) and standard titles as OTIO `GeneratorReference`s, their settings in `Resolve_OTIO` (a title's text among them). A Fusion title (Text+) comes out as a `MissingReference` with nothing in it: no program can do better than a gap for it.

- Keep each as a placeholder clip: a named, colored block that plays as a gap, can be moved, trimmed and deleted, and is written back as it came.
