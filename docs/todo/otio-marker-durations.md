# Marker durations through OTIO

**When:** next.

Markers in vidcut are one frame long (spec D40). OTIO and Resolve give markers a length (Resolve's timeline markers can span frames, and it writes them as OTIO marked ranges), and an import cuts them to one frame, so a round trip loses it.

- Keep a marker's length through import and export, and in `markers.list`, while the UI still shows it as a point.
- A clip's markers come in as timeline markers (Resolve writes them on the clip, in source time). Keeping them on their clip waits for clip markers in vidcut.
