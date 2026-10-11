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

What is still open is in [docs/todo/](todo/), one file per item.

## Milestones

- **Proxies** are in (decision D41, deviation X18). `vidcut proxy generate` writes an H.264 proxy on demand for any video or image sequence — it does not detect which codecs the GPU cannot play. The default longest side is 960. `vidcut set --proxies true` (or the Media Pool switch) plays a proxy in the preview when the source file still matches it. Export, frames, analysis and OTIO always read the originals. Proxy frames keep the source's timestamps, so the preview stays frame-exact, including image sequences that were too slow to decode as pictures. Stills are left as originals. Files live in `proxies/` beside the project.

Open work is in [docs/todo/](todo/), one file per item, each saying when it is wanted. Roughly: the interchange with Resolve first, then the smaller items, then macOS and Windows.
