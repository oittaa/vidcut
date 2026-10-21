# Roadmap

## Use cases

They decide what gets built and tested.

1. **Cut by script or AI, finish in Resolve.** Many clips cut largely automatically by a script or an AI agent: silences out, shots found, long stretches flagged. DaVinci Resolve's free version has no scripting and no MCP server, so the cutting happens in vidcut and the timeline goes to Resolve for color correction and effects.
2. **A quick edit on a modest laptop.** An intro and an outro around a Google Meet recording, its silent parts cut out. Even with Resolve Studio, on a low-powered laptop an app that starts fast and needs no GPU does it sooner.

**What they need:**
- Interchange with Resolve as easy as possible: moving an edit either way, or between computers, takes no fixing by hand.
- Running well without a GPU on a modest CPU, tested with real meeting recordings (variable-frame-rate H.264).
- Automatic cuts that are quick to tune: silence removal for speech, scene cuts for footage.

## What comes next

Open work is in [docs/todo/](todo/), one file per item, each with a **When:** line. With the interchange in place (use case 1), the smaller items come next, then macOS.

Finishing a simple video is its own list, in this order:

1. [Clip transform](todo/clip-transform.md): Dynamic Zoom, a slow push on a still, and the placement in Resolve's OTIO files.
2. [Voice isolation](todo/voice-isolation.md): a Meet recording is still noisy after its silences are cut.
3. [Ducking](todo/ducking.md): music that dips under speech.
