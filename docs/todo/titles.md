# Titles

**When:** not decided.

- vidcut has Text, lower-third and Scroll titles ([spec §8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)).
- The titles have words styled apart, shadows, zoom, rotation, opacity and fades.
- Position, zoom, rotation and opacity take keyframes ([keyframes](keyframes.md#titles)).
- vidcut reads and writes them in OTIO in Resolve's shape ([D67](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).

## Line Spacing and Tracking in the Inspector

- The measurements read files. Measure the range and the units of the two controls in Resolve's Inspector.
- vidcut holds the number of the file until then.
- vidcut accepts -4096 to 4096 until then. The reason is that Resolve draws a line spacing of 2000 from a file.
- Measure why Resolve's Inspector shows a tracking of 11 on an upper line when the file has none.
