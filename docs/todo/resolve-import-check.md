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
- Nested stacks (compound clips, nested timelines): the API import fails with `importSourceClips` on, for vidcut's files as for Resolve's own; with it off, a compound clip comes in with its clips offline and a nested timeline as an offline clip.
- EDL and FCPXML keep less than OTIO: the EDL one video track, no markers or audio tracks; the FCPXML (1.10) no markers.
- Odd: Resolve's API import of its own export of such a timeline failed with `importSourceClips` on, and rebuilt it with it off. The cause wasn't found (not its generators, Text+, compound clips, transitions or speeds).

**Studio 21.1.1, File > Import > Timeline:**
- `.otio`: as through the API; clips at 200%, 50%, reversed and frozen show vidcut's frames, but for the 200% clip starting at an odd source frame (deviation X9).
- `.otioz`: the media unpack into a folder named after the bundle, and link.
- Media elsewhere: found without asking in a renamed folder next to the file; otherwise Resolve asks for a folder, searches its subfolders, and links.

**Still open:** the free version.
