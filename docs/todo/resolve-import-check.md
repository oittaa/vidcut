# Check Resolve's import of our timelines

**When:** next (use case 1 in the [roadmap](../roadmap.md)).

Import vidcut's `.otio` and `.otioz` files into the current free and Studio versions of DaVinci Resolve. The interchange (decision D40) was written without a Resolve to test with; it follows the files Resolve itself exports.

- What should survive: tracks, J/L cuts, clip gain, markers, In/Out, still and sequence clips, sources with timecode.
- List what doesn't, and fix it on our side where the file allows.

**Studio 21, `.otioz`:** the media unpack and link; tracks and their names, linked picture and sound, J/L cuts, clip gain, markers (time, name, color), stills, image sequences and source timecode come through. In/Out and track faders don't: Resolve's files have no place for them (deviation X9).

**Studio 21.1.1 (2026-10-13, through its scripting API):**
- `.otio`: as in 21, plus the muted clip and the disabled clip, marker notes, and the start timecode (`01:00:00:00` and drop-frame both land on the right frame).
- `.otioz`: the media unpack into a folder named after the bundle, and link, image sequences too (two in one bundle, 120 and 30 pictures). One import hung once, with a blank dialog on screen; twelve more of bundles with sequences didn't, that same bundle among them, nor did a half-written one (refused at once).
- `.otiod`: the import takes its `content.otio`, not the folder, and finds the media.
- Back to vidcut: everything structural survives the round trip; In/Out, track faders and track settings don't, as Resolve drops vidcut's metadata (deviation X9).
- Resolve's own timelines (transitions, speed, effects, generators, compound clips): see the carrying items (`otio-carry-*`, `otio-compound-clips`, `otio-speed-carry`).
- EDL and FCPXML keep less than OTIO: the EDL one video track, no markers or audio tracks; the FCPXML (1.10) no markers.
- Odd: Resolve's API import of its own export of such a timeline failed with `importSourceClips` on, and rebuilt it with it off. The cause wasn't found (not its generators, Text+, compound clips, transitions or speeds).

**Still open:**
- The free version.
- A plain `.otio` whose media are elsewhere on that computer: Resolve's folder prompt.
- The GUI's File > Import > Timeline, against the API's results above.
