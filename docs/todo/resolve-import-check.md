# Check Resolve's import of our timelines

**When:** next (use case 1 in the [roadmap](../roadmap.md)).

Import vidcut's `.otio` and `.otioz` files into the current free and Studio versions of DaVinci Resolve. The interchange (decision D40) was written without a Resolve to test with; it follows the files Resolve itself exports.

- What should survive: tracks, J/L cuts, clip gain, markers, In/Out, still and sequence clips, sources with timecode.
- List what doesn't, and fix it on our side where the file allows.
