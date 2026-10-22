# Ducking in OpenTimelineIO files

**When:** with access to Resolve Studio, to export a timeline with a Ducker as OTIO. The ducking itself plays in vidcut ([spec D76](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).

## What is open

- How Resolve writes the Ducker of a track in an OTIO file is not known.
- vidcut keeps the ducking in the `vidcut` metadata of the track.
- Resolve does not keep this metadata. Thus a timeline that goes to Resolve loses its ducking.
- The notes of an OTIO export say that other editors will not hear it.

## What to do

1. Put a Ducker on a track in Resolve Studio, and export the timeline as OTIO.
2. Find its settings in the file.
3. If they are there, write vidcut's ducking in the same shape, and read that shape on import.
4. If Resolve leaves Track FX out of its OTIO files, delete this item.
