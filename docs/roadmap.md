# Roadmap

What vidcut is for, and what comes next.

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

## What comes next

Moving an edit in either direction, and between computers, should take no fixing by hand. Open work is in [docs/todo/](todo/), one file per item, each saying when it is wanted. Roughly: the interchange first (use case 1), then the smaller items, then macOS and Windows.
