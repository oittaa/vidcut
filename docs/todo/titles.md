# Titles

**When:** not decided.

- vidcut has Text, lower-third and Scroll titles ([spec §8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)).
- The titles have words styled apart, shadows, zoom, rotation, opacity and fades.
- vidcut reads and writes them in OTIO in Resolve's shape ([D67](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).

Open:

- **A stroke's step in Resolve:**
  - Resolve draws no stroke on a title that it reads from a file ([gotchas](../gotchas.md#resolves-otio-files)).
  - Thus, to find `STROKE_PER_TEXT`, set a stroke on one of Resolve's own titles in its Inspector.
  - Measure the stroke against the picture.
