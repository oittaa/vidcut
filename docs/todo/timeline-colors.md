# Timeline colors

**When:** next. The owner selected this on 2026-10-25.

The timeline gets a color space, as in Resolve. Every mix occurs in it. Each export converts the mixed frame to its output.

## Why

- An export mixes a dissolve in its own output colors (spec D50, deviation [X14](../deviations.md#x14-color-is-set-per-export-not-managed)).
- Thus one timeline dissolves in two ways:
  - An HDR export (D44) mixes the PQ or HLG code values.
  - An SDR export tone-maps each picture, then mixes. For example, the H.264 and ProRes presets always give SDR.
- The viewer always tone-maps each picture, then mixes ([design/playback.md](../design/playback.md)). Thus it agrees only with an SDR export.
- The viewer cannot know the export, because the user selects SDR or HDR for each export.
- An example: a dissolve from black to white at 203 cd/m².
  - Halfway, the HDR export shows about 27% of the screen's range. The viewer shows 50%.
  - This is calculated, not measured.

## What Resolve does

The manual:

- Resolve Color Management converts each clip from its Input Color Space to the Timeline Color Space. Then it converts the result to the Output Color Space (pp. 226–227).
- Automatic Color Management has a Color Processing Mode of SDR or HDR (p. 230).
  - Resolve selects the input color space of each clip from its file.
  - The user selects the output color space from a list.
- The manual presets (pp. 230–231):
  - SDR Rec.709 (the default), SDR P3 Broadcast, SDR P3 Cinema and SDR Rec.2020.
  - DaVinci Wide Gamut, HDR P3 Broadcast, HDR Rec.2020 and Custom.
- The output color space does not depend on the preset. Thus one timeline gives SDR and HDR deliverables (p. 232).
- The Style of a Cross Dissolve is Video (linear), Film (logarithmic), Additive, Subtractive, Highlights or Shadows (p. 748).

The scripting API: the project settings and the settings of each timeline (`useCustomSettings`) hold the color science and the timeline color space.

Measured in Resolve Studio 21.1.1 on 2026-10-25:

- A new project has the color science DaVinci YRGB, without color management. Its timeline and output color spaces are "Rec.709 (Scene)".
- The test timeline has two cuts of the PQ barcode fixture `bc5_hevc10_1280x720pq_30-1_g30b2_30f_noaudio_o0.mp4`, with a 10-frame Cross Dissolve on the cut.
- The frames were rendered to 16-bit TIFF.
- Each number is the level of a pixel that goes from black to white, as a share of white. The numbers are the 10 frames of the dissolve, in order.

| Color science, timeline, output | Frames 1 to 10 |
|---|---|
| DaVinci YRGB | 0.048 0.151 0.248 0.349 0.449 0.548 0.649 0.746 0.847 0.944 |
| Managed, Rec.2100 ST2084, Rec.2100 ST2084 | 0.075 0.229 0.344 0.445 0.533 0.614 0.693 0.770 0.853 0.942 |
| Managed, Rec.709 Gamma 2.4, Rec.2100 ST2084 | 0.072 0.222 0.336 0.437 0.525 0.606 0.686 0.763 0.848 0.940 |
| Managed, Rec.2100 HLG, Rec.2100 ST2084 | 0.176 0.290 0.383 0.466 0.545 0.619 0.695 0.769 0.851 0.942 |
| Managed, DaVinci WG/Intermediate, Rec.2100 ST2084 | 0.242 0.355 0.442 0.516 0.586 0.650 0.718 0.783 0.857 0.942 |

What the measurements show:

- Without color management, Resolve mixes the code values of the file in a straight line, at the weights of D50. An HDR export of vidcut mixes in the same way now.
- With color management, the timeline color space changes the mix.
- The output color space does not change the mix.
  - The test rendered a Rec.2100 ST2084 timeline to Rec.709 Gamma 2.4 too.
  - Each mixed level that was checked is the PQ level converted: (cd/m² ÷ 100)^(1/2.4), clipped at 1.
- With color management, the mix is not a straight line in the code values of a Rec.2100 ST2084 timeline. Its working space is not known.

## The plan

1. **A Timeline Color Space in the project** (`Settings`, `project.set`, the Project Settings window).
   - The choices: Auto, SDR (BT.709), HDR PQ and HDR HLG.
   - Auto is HDR when more than half of the timeline's pictures are HDR. This is the rule of D44 without the codec.
   - HDR is HLG when every HDR picture is HLG, else PQ.
2. **Every dissolve and wipe in the timeline color space.**
   - Each picture of the mix goes to the timeline color space first.
   - The mix is a straight line in the timeline's code values, at the weights of D50. Resolve without color management mixes in the same way.
   - The mixed frame then goes to the output.
3. **Exports.**
   - When the output has the timeline's colors, nothing changes.
   - When the output is different, only the frames with a mix change. A frame with one picture keeps the direct conversion, which gives the same result.
   - NVENC: `cuda::mix` mixes on the GPU. Then libplacebo converts the mix on the GPU (AGENTS.md rule 9).
   - The other GPUs mix on the CPU ([gpu-dissolves.md](gpu-dissolves.md)). There, the CPU also converts the mix.
4. **The viewer.**
   - With HDR timeline colors, the layers of a mix go into the float picture of `Mixing` as HDR code values. One draw converts that picture for the screen.
   - With SDR timeline colors, the viewer draws as it does now.
5. **Picture renders** (`frame render`, the timeline's tiles and storyboards) go through `export::make::Maker`. Thus they follow the exports.
6. **Docs.**
   - X14: the timeline has a color space.
   - D50: the mix is in the timeline's code values.
   - A new deviation: Resolve's color management mixes in a working space that is not known. vidcut mixes in a straight line in the timeline's code values.

## Open, for the owner

- Does the Auto of an export (D44) follow the timeline color space? Then an HDR timeline exports HDR where the codec carries HDR.
- Do placed pictures, titles and subtitles also mix in the timeline color space? They mix with the frame below in the output colors now.
- Which more of Resolve's color spaces does vidcut offer? Examples: P3, SDR Rec.2020, DaVinci Wide Gamut.
