# Opacity of a compound clip

**When:** not decided.

- vidcut fades each picture and title inside a compound clip by the opacity and the soft edges of the compound clip (`place::Placing`).
- Where two pictures inside it overlap, the lower picture thus shows through the upper picture.
- A picture inside it at the Normal mode takes the composite mode of the compound clip.
- Resolve possibly mixes the tracks of the compound clip first, and then fades or mixes the result. This is not measured.
- To do:
  - Measure it in Resolve Studio: a compound clip with two overlapping pictures, at an opacity of 50%.
  - Measure a compound clip with a composite mode, for example Multiply, over a picture below it.
  - If Resolve fades the mix, mix the tracks of the compound clip first. Then fade the mix, in the viewer and in the export.
