# A primary grade

**When:** not decided.

Each clip has one primary correction, on the Color page ([color-page.md](color-page.md)).

**Why:**

- The Primaries palette holds these controls (pp. 3146–3147).
- A program grade still finishes in Resolve ([roadmap](../roadmap.md)). Whether an OpenTimelineIO file keeps a grade is not measured.

**In the code:**

- `model::Grade` holds the values. `fs_grade` applies them ([export](../design/export.md#the-grade)).
- The grade runs after the effects of the clip and before the composite mode and the opacity (§13).
- The scopes read the canvas after the mix ([gpu](../design/gpu.md#the-scopes)). Thus they show the grade.

**To do:**

- Temp and Tint: at Temp 1000, the result is 0.00065 off at an input of 1. Find the rule that makes the matrix exact.
- A timeline grade, with the same math. It runs on the mixed picture, before subtitles and the encode.
- Measure whether an OpenTimelineIO file keeps a grade, in each direction.
- With a grade, the export measures light on the graded frames ([X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).
- The values take keyframes once effect keyframes exist ([effects-library.md](effects-library.md)).

**Not this item:**

- The display LUT ([luts.md](luts.md)).
- A qualifier, after this primary.
- OpenColorIO, after the extra color spaces ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
