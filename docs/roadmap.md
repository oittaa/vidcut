# Roadmap

What vidcut is for, and what comes next. Finished milestones are listed in the [README](../README.md#milestones).

## Use cases

These guide what gets built and what gets tested.

1. **Cut by script or AI, finish in Resolve.**
   - Someone has many clips and wants them cut largely automatically: silences out, shots found, long stretches flagged, by a script or an AI agent.
   - DaVinci Resolve's free version has no scripting and no MCP server, so they do this quick cutting in vidcut.
   - They then import the timeline into Resolve for color correction and effects.
2. **A quick edit on a modest laptop.**
   - Someone has a Google Meet recording and wants an intro and an outro around it, with the silent parts in the middle cut out.
   - They may own Resolve Studio, but on a low-powered laptop a light app that starts fast and needs no GPU does this sooner.

**What they need from vidcut:**
- Interchange with Resolve that is as easy as possible (below).
- Running well without a GPU on a modest CPU. Meeting recordings are variable-frame-rate H.264: test with real ones.
- Automatic cuts that are quick to tune: silence removal for speech, scene cuts for footage.

## To do: interchange with Resolve

Moving an edit between vidcut and Resolve, in either direction and between computers, should take no fixing by hand.

**Done** (deviation X9, decision D40): `.otio`, `.otioz` and `.otiod` both ways; plain paths and timecode source ranges as Resolve writes them; link groups keeping audio with its video; Resolve's clip volume, channel map, track lock and solo, clip markers and offline media read; relative paths and a project folder moved with its timeline found by themselves; files checked against OpenTimelineIO's Python library.

- **Check Resolve's import of our files** with the current free and Studio versions (no Resolve to test with when the above was written; it follows Resolve's own exports).
  - What should survive: tracks, J/L cuts, clip gain, markers, In/Out, still and sequence clips, sources with timecode.
  - List what doesn't, and fix it on our side where the file allows.
- **Portable project paths.** Projects store absolute media paths. Store them relative to the project file when the media sit in or under its folder, so a folder holding the project and its media can move to another computer.
- **Relinking steps in the README** for media that moved between computers (Resolve's folder search, vidcut's `--search`).
- **Other formats if OTIO falls short.** Resolve also imports FCPXML, FCP7 XML and EDL. Add one only if a check above shows OTIO losing something these keep.

## Milestones

- **Proxies** are in (decision D41, deviation X18). `vidcut proxy generate` writes an H.264 proxy on demand for any video or image sequence — it does not detect which codecs the GPU cannot play. The default longest side is 960. `vidcut set --proxies true` (or the Media Pool switch) plays a proxy in the preview when the source file still matches it. Export, frames, analysis and OTIO always read the originals. Proxy frames keep the source's timestamps, so the preview stays frame-exact, including image sequences that were too slow to decode as pictures. Stills are left as originals. Files live in `proxies/` beside the project.

- **Candidates, roughly in this order:**
  - **Interchange with Resolve:** the list above.
  - **Smaller items** from [gui.md §12](decisions/gui.md): a render queue, per-channel meters, Source Tape marks across clips, dragging bins into bins.
  - **macOS and Windows.**
