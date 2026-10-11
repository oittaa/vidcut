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

- **Portable paths.** Projects and `.otio` exports store absolute media paths today.
  - Store paths relative to the project file (or the `.otio` file) when the media sit in or under its folder, and absolute paths otherwise.
  - A folder holding the project and its media can then move to another computer, or another operating system, and still open.
- **Check Resolve's import of our `.otio`** with the current free and Studio versions.
  - What should survive: tracks, J/L cuts, clip gain, markers, In/Out, and track names and volumes.
  - List what doesn't, and fix it on our side where the file allows.
- **Relinking.** Write media references that Resolve's relink (by file name) finds when paths differ, and document the steps in the README.
- **The way back.** Importing Resolve's `.otio` exports into vidcut works since M5 (deviation X9). Check it against current Resolve versions.
- **Other formats if OTIO falls short.** Resolve also imports FCPXML, FCP7 XML and EDL. Add one only if a check above shows OTIO losing something these keep.
- **A bundle.** Export the timeline together with its media (copied, or linked) in one folder, like OpenTimelineIO's `.otiod` bundles, to hand an edit to another computer.

## Milestones

- **Candidates, roughly in this order:**
  - **Interchange with Resolve:** the list above.
  - **Proxies,** for footage the GPU can't decode (10-bit 4:2:2, ProRes, DNxHR, 8K), for slow storage, and for image sequences of big pictures (a 4K 16-bit PNG takes about 100 ms to decode, so such a sequence can't play in real time).
    - Resolve generates them from the Media Pool and switches between proxies and originals; export always reads the originals (pp. 207–218).
    - Ours would keep the source's frame timestamps, so the preview stays frame-exact.
  - **Smaller items** from [gui.md §12](decisions/gui.md): a render queue, per-channel meters, dragging bins into bins.
  - **macOS and Windows.**
