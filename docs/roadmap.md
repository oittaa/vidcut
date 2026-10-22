# Roadmap

## Use cases

The use cases decide what is built and tested.

1. **Cut by script or AI, finish in Resolve.**
   - A script or an AI agent cuts many clips, largely automatically: it removes silences, finds shots and flags long stretches.
   - DaVinci Resolve's free version has no scripting and no MCP server.
   - Thus the cutting happens in vidcut, and the timeline goes to Resolve for color correction and effects.
2. **A quick edit on a modest laptop.**
   - An example: an intro and an outro around a Google Meet recording, with its silent parts cut out.
   - Even with Resolve Studio, an app that starts fast and needs no GPU does this sooner on a low-powered laptop.

**What they need:**

- Interchange with Resolve that is as easy as possible: moving an edit either way, or between computers, needs no fixing by hand.
- Running well without a GPU on a modest CPU, tested with real meeting recordings (variable-frame-rate H.264).
- Automatic cuts that are quick to tune: silence removal for speech, and scene cuts for footage.

## What comes next

- Open work is in [docs/todo/](todo/), one file for each item. Each file has a **When:** line.
- The interchange (use case 1) is in place. The smaller items come next, then macOS.
