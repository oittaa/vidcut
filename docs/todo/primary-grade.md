# A primary grade

**When:** not decided.

Each clip has one primary correction, on the Color page ([color-page.md](color-page.md)). Each timeline also has one.

**Why:**

- The Primaries palette holds these controls (pp. 3146–3147).
- A program grade still finishes in Resolve ([roadmap](../roadmap.md)).
- The OpenTimelineIO files of Resolve keep no timeline grade ([spec §8.20](../spec.md#820-primaries-palette-pp-118-31413159)). Whether they keep a clip grade is not measured.

**In the code:**

- `model::Grade` holds the values. `fs_grade` applies them ([export](../design/export.md#the-grade)).
- The grade runs after the effects of the clip and before the composite mode and the opacity (§13).
- The timeline grade runs on the mixed canvas, before the subtitles and the output LUT ([export](../design/export.md#the-grade)).
- The scopes read the canvas after the mix ([gpu](../design/gpu.md#the-scopes)). Thus they show the grade.

**To do:**

- Temp and Tint: at Temp 1000, the result is 0.00065 off at an input of 1. Find the rule that makes the matrix exact.
- Measure whether an OpenTimelineIO file of Resolve keeps a clip grade, in each direction.
- The values take keyframes once effect keyframes exist ([effects-library.md](effects-library.md)).

**Not this item:**

- The display LUT ([luts.md](luts.md)).
- A qualifier, after this primary.
- OpenColorIO, after the extra color spaces ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
