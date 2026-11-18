# A primary grade

**When:** not decided.

Each clip has a grade of nodes, on the Color page ([color-page.md](color-page.md)). Each timeline also has one. Each corrector node holds one primary correction.

**Why:**

- The Primaries palette holds these controls (pp. 3146–3147).
- A program grade still finishes in Resolve ([roadmap](../roadmap.md)).
- The OpenTimelineIO files of Resolve keep no grade, of a clip or of a timeline ([spec §8.20](../spec.md#820-primaries-palette-pp-118-31413159)).

**In the code:**

- `model::Grade` holds the nodes, and `model::Corrector` holds the values of a corrector. `fs_grade` applies them, and `fs_grade_mix` mixes ([export](../design/export.md#the-grade)).
- The grade runs after the effects of the clip and before the composite mode and the opacity (§13).
- The timeline grade runs on the mixed canvas, before the subtitles and the output LUT ([export](../design/export.md#the-grade)).
- The scopes read the canvas after the mix ([gpu](../design/gpu.md#the-scopes)). Thus they show the grade.

**To do:**

- With a grade, the export measures light on the graded frames ([X19](../deviations.md#x19-hdr-metadata-comes-from-the-sources)).

**Not this item:**

- The display LUT ([luts.md](luts.md)).
- A qualifier, after this primary.
- OpenColorIO, after the extra color spaces ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
