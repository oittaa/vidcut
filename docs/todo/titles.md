# Titles

**When:** not decided.

vidcut has Text titles and the three lower thirds ([spec §8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)), read from and written to OTIO in Resolve's shape ([D67](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). Open:

- **Measure Resolve's units:** render titles of known point sizes, `strokeSize` and `backgroundHeight` in Resolve and measure them against the picture, to set `POINTS_PER_PICTURE` and `STROKE_PER_TEXT` and the background's height, and check that `position` counts from the bottom. Check in Resolve that a title vidcut writes comes back as a title.
- **More kinds:** Scroll, Text+ and Fusion titles (p. 1178). Resolve writes Text+ as a `MissingReference` with nothing of the title in it, so it can't come over.
- **Rich text:** words of one title styled apart, and a size for each of a lower third's lines (p. 1178).
- **Drop shadows** (p. 764), and keeping a Resolve title's shadow, zoom and rotation when it is read.
- **Transform, Cropping, Speed and Composite** on title clips (pp. 763, 1176), and the viewer's onscreen controls (pp. 763–764).
- **Animation and keyframes**, and dissolves that fade a title: a dissolve at a title's edge leaves it whole ([D65](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).
- **A Titles browser tab** with Hover Scrub Preview (p. 762).
- **Thumbnails** of title clips on the timeline.
