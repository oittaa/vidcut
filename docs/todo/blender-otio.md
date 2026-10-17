# Blender

**When:** once Blender's own OTIO import and export are released (export is Blender pull request #158562, import #161207; Blender 5.2 has neither).

`pip install bpy` gives Blender as a Python module to test with. Try both ways, watching:
- Paths: vidcut writes plain ones (spec D40), which OpenTimelineIO's own tools misread on Windows (deviation X9).
- J/L cuts as audio clips offset from the video: Blender splits mixed channels, and doesn't know Resolve's link groups.
- Image sequences, the start timecode, and transitions and speed effects (carried, deviation X9).
