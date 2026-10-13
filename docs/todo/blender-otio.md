# Blender

**When:** when Blender's own OTIO import and export are released.

Blender 5.2 has neither (nor EDL); `bpy` from PyPI is the same Blender as a module. Its OTIO support is work in progress built on the OpenTimelineIO library (Google Summer of Code 2026: export pull request #158562, import #161207), mapping crossfades and wipes to transitions and speed to an effect, and keeping markers and metadata. The VSE IO add-on (2021) looks unmaintained. Until then the OpenTimelineIO library is the check (`tests/cli.rs`, in CI on Linux and Windows).

Then try both ways, watching:
- Paths: vidcut writes plain ones (spec D40), which OpenTimelineIO's own tools misread on Windows (deviation X9).
- J/L cuts as audio clips offset from the video: Blender splits mixed channels, and doesn't know Resolve's link groups.
- Image sequences, the start timecode, and its transitions and speed effects (the carrying items).
