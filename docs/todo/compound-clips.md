# Compound clips: In and Out, and Decompose in Place

**When:** not decided.

`edit.compound` makes a compound clip from timeline clips ([spec §8.18](../cut-page-spec.md#818-compound-clips-pp-10521054-1126-measured-in-resolve-studio-2120), [deviation X163](../deviations.md#x163-a-compound-clip-takes-its-clips-whole)). These parts need a measurement in the window of Resolve, because the scripting call does not reach them.

**Why:**

- A range on all tracks is a fast way to nest a scene.
- Decompose in Place has choices that vidcut does not know yet.

**To do:**

- Measure Convert In and Out to Compound Clip (manual p. 1053). It is on the Edit page.
- Find the separate context action that creates a compound clip from the In–Out range. The 21.1 manual and the notes of the later versions do not name it.
- Measure Decompose in Place (manual p. 1054): the track of each clip, and clips that are in the way.
- Make a dissolve play inside a compound clip. `render::expanded` removes transitions there, so a dissolve plays as a cut until a decompose.
