# Blender

**When:** when Blender's own OTIO import and export are released.

Blender 5.2 has no OTIO or EDL import and export (`bpy` from PyPI is the same Blender as a module). Its OTIO support is in progress (Blender pull requests #158562, export, and #161207, import), built on the OpenTimelineIO library: crossfades and wipes as transitions, speed as an effect, markers and metadata kept. Until it ships, OpenTimelineIO's library is the check (`tests/cli.rs`, in CI on Linux and Windows).

Then try both ways, watching:
- Paths: vidcut writes plain ones (spec D40), which OpenTimelineIO's own tools misread on Windows (deviation X9).
- J/L cuts as audio clips offset from the video: Blender splits mixed channels, and doesn't know Resolve's link groups.
- Image sequences, the start timecode, and its transitions and speed effects (the carrying items).
