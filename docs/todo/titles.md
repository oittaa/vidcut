# Titles

**When:** not decided.

- vidcut has Text, lower-third and Scroll titles ([spec §8.14](../spec.md#814-titles-pp-762764-11761178)).
- The titles have words styled apart, shadows, zoom, rotation, opacity and fades.
- Position, zoom, rotation, opacity, the shadow and the background take keyframes ([spec §8.14](../spec.md#814-titles-pp-762764-11761178)).
- vidcut reads and writes them in OTIO in Resolve's shape ([D67](../spec.md#12-decisions-on-gaps-and-contradictions)).

## Position

- vidcut draws the letters of a Text title 0.5 × zoom pixels left of Resolve, at each anchor.
  - Down, vidcut is 0.5 pixels lower at zoom 1. It is 0.75 pixels higher at zoom 1.5, and 1 pixel higher at zoom 2.
  - Measured in Resolve Studio 21.2 with Arial 96, at the left, middle and right anchors. Find the stage that makes this offset.

## Zoom X and Zoom Y

- **Stretched text:** vidcut lays out the text at Zoom Y, then stretches it across by Zoom X / Zoom Y ([spec §8.14](../spec.md#814-titles-pp-762764-11761178)).
  - Run: a Text title at Zoom X 1.5 and Zoom Y 1, at 1920×1080 and 3840×2160. Measure the width of the letters, the wrap and the outline.

## Tracking

- Measure a tracking with a fraction of a pixel at a zoom. vidcut takes the step of 1/64 of a pixel before the zoom.
- Find the pad of a block when a letter at an end has ink outside its advance.
  - In Arial 96 at tracking 0, "Ab", "AV", "AH" and "HV" are 2 pixels wider than the rule. "jH" is 8 pixels wider, and "Hf" 9.
  - Gabriola has the same pad. "H" is 1, 5 and 9 pixels wider at sizes 48, 96 and 192.
  - The pad gets smaller at large negative trackings.

## Font heights

- **Scroll letters:** in run T5, the letters of a Scroll have an em of 4/3 of its size. vidcut draws an em of the size.
  - The font of that Scroll was picked in the Inspector. Its file gives the font in points: `Arial,8.25,-1,…`.
  - vidcut writes the font in pixels: `Arial,-1,58,…`. Measure the letters of a Scroll from each kind of file.
- **Faces of a variable font:** vidcut does not find a face by its own family name, for example "Bahnschrift Light" or "Bahnschrift SemiCondensed".
  - Windows lists these faces as families. Resolve lists 12 families of Bahnschrift, one for each named face, and draws them.
  - Resolve's file writes the family of the face, with a `font-weight` of 200 for Light and 504 for SemiBold.
  - vidcut draws another font for each of these names. At size 96, the ink of "Bahnschrift HHH" is then 685 pixels wide in each face.
- **Fallback font:** a letter that the font does not have comes from another font.
  - In Arial at size 192, `Ⅷ` moves the words down by 35 pixels in vidcut and 9 pixels in Resolve. `Ⓐ` moves them 33 pixels in vidcut and 0 in Resolve.
  - The two programs draw these letters in different fonts. Find which font Resolve takes.

## Edit in place

- **Lower third:** Resolve edits only the line under the pointer, inside an outline of that line.
  - Each line of Resolve's lower third has its own Rich Text. vidcut keeps the two lines in one text.
- **Tab:** in Resolve, Tab types a tab character. Measure the width of a tab in a title, then let Tab type it.
- **Spell check:** Resolve draws a red wavy line under the words that its spell check does not know. vidcut has no spell check.
- **Bursts:** measure what ends one burst of typing in Resolve: a pause, a space or another key.
