# Blender

**When:** when Blender's own OTIO import and export are released.

Blender 5.2 has neither. Export is in Blender pull request #158562 and import in #161207. `pip install bpy` gives Blender as a Python module to test with.

Then try both ways, watching:
- Paths: vidcut writes plain ones (spec D40), which OpenTimelineIO's own tools misread on Windows (deviation X9).
- J/L cuts as audio clips offset from the video: Blender splits mixed channels, and doesn't know Resolve's link groups.
- Image sequences, the start timecode, and its transitions and speed effects (carried, deviation X9).
