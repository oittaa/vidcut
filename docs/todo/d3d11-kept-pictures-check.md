# Direct3D 11's kept pictures on Windows

**When:** next time on a Windows computer with an AMD or Intel GPU. Direct3D 11 decodes there; NVIDIA's GPUs decode on NVDEC.

Each picture the player keeps from Direct3D 11 is copied into a texture in frames of its own, freed with the picture (preview.md §3). That code is only compiled by CI and has never run on a Windows GPU. Check that pictures still show, and that the GPU's memory stops growing where clips of two sizes take turns.

In a new project with a 4K clip and a 1080p clip (H.264 or HEVC), one after the other on the timeline:

1. Open Task Manager, Details tab, and add the Dedicated GPU memory column.
2. Right-click the viewer and choose Stats for Nerds.
3. Play across the cut with Space, then click back into the first clip and play across it again, five times.

Expected:
- The picture plays as before, and the Decoding group says Direct3D 11.
- vidcut's dedicated GPU memory rises to about the Kept group's video memory plus a few hundred MB, then stays there, not climbing with each pass.

If it climbs, note its value after each pass, the Kept group's numbers, and the GPU and its driver version (Task Manager, Performance tab).
