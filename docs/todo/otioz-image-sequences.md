# Resolve hangs on bundles with image sequences

**When:** next.

DaVinci Resolve Studio 21.1.1 stops responding when it imports an `.otioz` that holds an image sequence: it extracts `version.txt` and hangs, idle, until it is killed (seen through its scripting API, 2026-10-13; the GUI's import uses the same importer and wasn't tried). The same bundle without the sequence imports and links every clip, and a plain `.otio` with the sequence imports fine. The [import check](resolve-import-check.md) saw bundled sequences work in 21.0, so this looks like a regression.

- Exporting an `.otioz` or `.otiod` with an image sequence warns that Resolve 21.1.1 hangs on it, and suggests `.otio` with the media in place, or rendering the sequence to a file.
- Report it to Blackmagic with a minimal bundle (one sequence clip), and check again with each Resolve release; drop the warning when it is fixed.
