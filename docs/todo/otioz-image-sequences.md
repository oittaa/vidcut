# Resolve hangs on bundles with image sequences

**When:** with each Resolve release.

Studio 21.1.1 hangs importing a bundle that holds an image sequence (deviation X9; vidcut's export notes it). It extracts `version.txt` and stops, idle, until it is killed (seen through its scripting API, 2026-10-13); the [import check](resolve-import-check.md) saw bundled sequences work in 21.0.

- Report it to Blackmagic with a minimal bundle (one sequence clip).
- Check again with each release; when it is fixed, drop the note and X9's line.
