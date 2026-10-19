# Titles

**When:** not decided.

vidcut has Text, lower-third and Scroll titles with words styled apart, shadows, zoom, rotation, opacity and fades ([spec §8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)), read from and written to OTIO in Resolve's shape ([D67](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)). Open:

- **A stroke's step in Resolve:** Resolve draws no stroke on a title it reads from a file ([gotchas](../gotchas.md#resolves-otio-files)), so `STROKE_PER_TEXT` needs a stroke set on one of Resolve's own titles in its Inspector, measured against the picture.
