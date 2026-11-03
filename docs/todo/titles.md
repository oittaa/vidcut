# Titles

**When:** not decided.

- vidcut has Text, lower-third and Scroll titles ([spec §8.14](../cut-page-spec.md#814-titles-pp-762764-11761178)).
- The titles have words styled apart, shadows, zoom, rotation, opacity and fades.
- Position, zoom, rotation and opacity take keyframes ([keyframes](keyframes.md#titles)).
- vidcut reads and writes them in OTIO in Resolve's shape ([D67](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)).

## Title Caps in the Inspector

- Resolve draws no change from `text-transform:capitalize` in a file. Thus the measurements have no Title Caps.
- Set Title Caps in Resolve's Inspector on small and capital letters, on `ab-cd` and on `e'f`.
- Measure which letters change, and read the form that Resolve writes in its file.

## Line Spacing and Tracking in the Inspector

- Measure the range of the two controls in Resolve's Inspector, and the unit of Line Spacing.
- vidcut holds the number of the file for Line Spacing until then.
- vidcut accepts -4096 to 4096 until then. The reason is that Resolve draws a line spacing of 2000 from a file.
- Measure Tracking on bold letters, and with `<sup>`, `<sub>` and small capitals. vidcut uses the width of X in the bold font, at the size before the shift or the small capitals.
- Measure a tracking with a fraction of a pixel at a zoom. vidcut takes the step of 1/64 of a pixel before the zoom.
- Find why some blocks with a negative tracking are up to 1 pixel from the rule, as "Ab" in Arial 96.
- Measure `<u>`, `<o>` and `<s>` with Tracking. vidcut draws them across the last letter's space. Also measure them on part of a line, and on `<sup>`.
- Measure `<sup>` and `<sub>` beside other letters on a line. The measurements have them on a whole line only.

## Font heights

- **Scroll:** in Resolve, the line distance of a Scroll does not follow the sum of the three heights.
  - Arial and Times New Roman have the same sum, and the distances differ by 3 pixels at the same size.
  - The distance follows ascent plus descent, with no gap, in these two fonts and in Calibri. Measure it at more sizes, then replace the rule with 4/3.
- **Variable fonts:** a stored title with only the family name draws Bahnschrift narrower in Resolve than in vidcut. Choose the font and its face in the Inspector, and measure which face Resolve takes.
- **Family names:** vidcut draws `Sitka Text` in its sans-serif font, though the font is installed. Find why vidcut does not find that family.
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
