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

## Anchor of a lower third and a scroll

- In vidcut, only a Text title has `anchor`.
- In Resolve, each line of a lower third and a Scroll have an anchor too.
- A Scroll's anchor works on a text box as wide as the picture (measured in Resolve Studio 21.1.2):
  - The left column puts the left edge of the lines at the middle of the picture.
  - The right column puts the lines off the picture.
  - The row moves the lines by half of a text height, up or down.
- **Why it is open:** the shape of this control in Resolve's Inspector is not checked.
