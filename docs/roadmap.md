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

Finishing a simple video here is a separate list. The order is what a meeting cut needs first, then what a picture move needs, then what cannot ride along in an OTIO file or what is too heavy for the laptop preview.

### Next

1. [Clip transform and crop](todo/clip-transform.md). Position, zoom, rotation, crop and keyframes place a picture, and speed ramps and stabilization write the same curves.
2. [Audio cleanup](todo/audio-cleanup.md). A Meet recording is still noisy after the silences are cut, and the denoiser runs on a laptop CPU.
3. [Clip and track audio](todo/audio-effects.md). Ducking, a voice EQ, a high-pass, a compressor and a de-esser use the speech detection and the loudness export already have, and they ship with no model.
4. [Freeze frame and holds](todo/freeze-frame.md). A whole clip already freezes; one chosen frame, and a hold at the head or the tail, is the rest, and it round-trips.

### Later

5. [Basic color](todo/color.md). Exposure, white balance and a LUT finish a picture here, and a grade does not survive an OTIO file into Resolve.
6. [Speed ramps](todo/speed-ramps.md). A speed that changes reuses the transform's curves. Constant speed already exists, and interpolation past the nearest frame stays off a laptop preview.
7. [Stabilization](todo/stabilization.md). The smoothed motion is a per-frame transform, so it waits on that, and the vid.stab releases in distribution are GPL, so they are not the default.
