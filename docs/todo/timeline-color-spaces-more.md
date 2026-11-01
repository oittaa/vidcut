# More timeline and output color spaces

**When:** not decided.

The timeline color space ([spec D50](../cut-page-spec.md#12-decisions-on-gaps-and-contradictions)) has SDR (BT.709), HDR PQ and HDR HLG. Exports write only these colors.

## What Resolve has

- The presets of Resolve Color Management also have P3, Rec.2020 SDR and DaVinci Wide Gamut (pp. 230–231).
  - SDR P3 Broadcast, SDR P3 Cinema and SDR Rec.2020.
  - DaVinci Wide Gamut and HDR P3 Broadcast.
- The output color space is a separate list (p. 232).

## Why later

- vidcut has no grading. Thus a timeline color space changes only the mixes.
- The primary grade is separate ([primary-grade.md](primary-grade.md)). These spaces stay the work of this file.
- A P3 or Rec.2020 SDR timeline is useful with an output in the same colors. Exports have no such output.
- Each new color space needs its math in the one draw: `crates/vidcut-av/src/screen/picture.wgsl`, with `color::shader_colors` and `color::output_gamut`.
- Each new output also needs its tags (`color::tag`).

## To do

1. P3-D65 and Rec.2020 SDR, as timeline and output color spaces.
   - The tags of each output, and its HDR metadata (D46).
2. DaVinci Wide Gamut and DaVinci Intermediate. An example use: intermediate files that Resolve grades.
3. Check each conversion on the barcode fixtures.
