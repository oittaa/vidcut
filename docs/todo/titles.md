# Titles

**When:** not decided.

- vidcut has Text, lower-third and Scroll titles ([spec §8.14](../spec.md#814-titles-pp-762764-11761178)).
- The titles have words styled apart, shadows, zoom, rotation, opacity and fades.
- Position, zoom, rotation and opacity take keyframes ([spec §8.14](../spec.md#814-titles-pp-762764-11761178)).
- vidcut reads and writes them in OTIO in Resolve's shape ([D67](../spec.md#12-decisions-on-gaps-and-contradictions)).

## Tracking

- Measure a tracking with a fraction of a pixel at a zoom. vidcut takes the step of 1/64 of a pixel before the zoom.
- Find why some blocks with a negative tracking are up to 1 pixel from the rule, as "Ab" in Arial 96.

## Font heights

- **Scroll:**
  - Measure the line distance at a size that is not a multiple of 3. vidcut rounds 4/3 of the size to the nearest whole pixel.
  - vidcut keeps the exact size of a Scroll, because its rounding is not measured. Measure a Scroll at a size with a fraction.
- **Faces of a variable font:** vidcut does not find a face by its own family name, for example "Bahnschrift Light" or "Bahnschrift SemiCondensed".
  - Windows lists these faces as families, and Resolve draws them.
  - vidcut draws another font for each of these names. At size 96, the ink of "Bahnschrift HHH" is then 685 pixels wide in each face.
- **Gabriola:** the letters are 0.3 pixels left of Resolve, and the rows match. Find the reason.
- **Fallback font:** a letter that the font does not have comes from another font.
  - In Arial at size 192, `Ⅷ` moves the words down by 35 pixels in vidcut and 9 pixels in Resolve. `Ⓐ` moves them 33 pixels in vidcut and 0 in Resolve.
  - The two programs draw these letters in different fonts. Find which font Resolve takes.

## Background in the Inspector

- The measurements of `shape` read files. Measure the range and the units of these controls in Resolve's Inspector.
- vidcut uses the numbers of the file until then.
