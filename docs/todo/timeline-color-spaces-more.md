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
- A P3 or Rec.2020 SDR timeline is useful with an output in the same colors. Exports have no such output.
- Each new color space needs its math in four places:
  - The CPU (`CpuColors` in `crates/vidcut-av/src/color.rs`).
  - The shaders that libplacebo runs (`color::shader`).
  - The zscale filters of the CPU path (`color::conversion`).
  - The shader of the viewer and the export (`crates/vidcut-av/src/screen/picture.wgsl`).
- The zscale documentation of FFmpeg 9.0.1 lists no P3 primaries and no DaVinci Intermediate transfer. Thus the CPU path needs math of vidcut's own for them.

## To do

1. P3-D65 and Rec.2020 SDR, as timeline and output color spaces.
   - The tags of each output, and its HDR metadata (D46).
2. DaVinci Wide Gamut and DaVinci Intermediate. An example use: intermediate files that Resolve grades.
3. Compare each conversion with the CPU's pictures on the barcode fixtures.
