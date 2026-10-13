# Check Resolve's import of our timelines

**When:** next (use case 1 in the [roadmap](../roadmap.md)).

Import vidcut's `.otio` and `.otioz` files into the current free and Studio versions of DaVinci Resolve. The interchange (decision D40) was written without a Resolve to test with; it follows the files Resolve itself exports.

- What should survive: tracks, J/L cuts, clip gain, markers, In/Out, still and sequence clips, sources with timecode.
- List what doesn't, and fix it on our side where the file allows.

**Studio 21, `.otioz`:** the media unpack and link; tracks, linked picture and sound, J/L cuts, clip gain, markers (time, name, color), stills, image sequences and source timecode come through. In/Out and track faders don't: Resolve's files have no place for them (deviation X9).

**Still open:**
- Track names as X9 gives them (Resolve's Mixer shows them).
- The free version.
- A plain `.otio` whose media are elsewhere on that computer: Resolve's folder prompt.
