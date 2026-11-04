# Titles

**When:** not decided.

- vidcut has Text, lower-third and Scroll titles ([spec §8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)).
- The titles have words styled apart, shadows, zoom, rotation, opacity and fades.
- Position, zoom, rotation and opacity take keyframes ([keyframes](keyframes.md#titles)).
- vidcut reads and writes them in OTIO in Resolve's shape ([D67](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).

## Tracking

- Measure a tracking with a fraction of a pixel at a zoom. vidcut takes the step of 1/64 of a pixel before the zoom.
- Find why some blocks with a negative tracking are up to 1 pixel from the rule, as "Ab" in Arial 96.

## Font heights

- **Scroll:** in Resolve, the line distance of a Scroll does not follow the sum of the three heights.
  - Arial and Times New Roman have the same sum, and the distances differ by 3 pixels at the same size.
  - The distance follows ascent plus descent, with no gap, in these two fonts and in Calibri. Measure it at more sizes, then replace the rule with 4/3.
  - vidcut keeps the exact size of a Scroll, because its rounding is not measured. Measure a Scroll at a size with a fraction.
- **Variable fonts:** a stored title with only the family name draws Bahnschrift narrower in Resolve than in vidcut. Choose the font and its face in the Inspector, and measure which face Resolve takes.
- **Gabriola:** the letters are 0.3 pixels left of Resolve, and the rows match. Find the reason.

## Background in the Inspector

- The measurements of `shape` read files. Measure the range and the units of these controls in Resolve's Inspector.
- vidcut uses the numbers of the file until then.

## Anchor of a lower third and a scroll

- In vidcut, only a Text title has `anchor`.
- In Resolve, each line of a lower third and a Scroll have an anchor too.
- A Scroll's anchor works on a text box as wide as the picture (measured in Resolve Studio 21.1.2):
  - The left column puts the left edge of the lines at the middle of the picture.
  - The right column puts the lines off the picture.
  - The row moves the lines by half of a text height, up or down.
- **Why it is open:** the shape of this control in Resolve's Inspector is not checked.
