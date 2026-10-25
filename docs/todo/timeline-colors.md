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
- The test timelines use the PQ barcode fixture `bc5_hevc10_1280x720pq_30-1_g30b2_30f_noaudio_o0.mp4`. Its white is at 205 cd/m².
- The frames were rendered to 16-bit TIFF.

A 10-frame Cross Dissolve on a cut between two parts of the clip:

- Each number is the level of a pixel that goes from black to white, as a share of white. The numbers are the 10 frames of the dissolve, in order.

| Color science, timeline, output | Frames 1 to 10 |
|---|---|
| DaVinci YRGB | 0.048 0.151 0.248 0.349 0.449 0.548 0.649 0.746 0.847 0.944 |
| Managed, Rec.2100 ST2084, Rec.2100 ST2084 | 0.075 0.229 0.344 0.445 0.533 0.614 0.693 0.770 0.853 0.942 |
| Managed, Rec.709 Gamma 2.4, Rec.2100 ST2084 | 0.072 0.222 0.336 0.437 0.525 0.606 0.686 0.763 0.848 0.940 |
| Managed, Rec.2100 HLG, Rec.2100 ST2084 | 0.176 0.290 0.383 0.466 0.545 0.619 0.695 0.769 0.851 0.942 |
| Managed, DaVinci WG/Intermediate, Rec.2100 ST2084 | 0.242 0.355 0.442 0.516 0.586 0.650 0.718 0.783 0.857 0.942 |

Other mixes:

- **Opacity.** The clip, flipped, at 50% Opacity on a track above the clip.
  - White over black is at 0.499 of white without color management. It is at 0.574 with a managed Rec.2100 ST2084 timeline.
  - 0.574 is also where the managed dissolve is halfway.
- **A title.** A Text title over black.
- **Subtitles.** An SRT cue over black, burned in at the render.
- White text of a title and of a subtitle is at 100 cd/m² in a Rec.2100 ST2084 timeline. The project setting `graphicsWhiteLevel` is 100.
  - vidcut puts this white at 203 cd/m² (D35, D61). This is the same as Resolve's "203 nits" option.

The tests also rendered each managed Rec.2100 ST2084 timeline to Rec.709 Gamma 2.4:

- Each pixel between black and white is the PQ level converted: (cd/m² ÷ 100)^(1/2.4), clipped at 1.
- This is true for the dissolve, the opacity, the edges of the title and the edges of the subtitles.

What the measurements show:

- Without color management, Resolve mixes the code values in a straight line. A dissolve uses the weights of D50. An HDR export of vidcut mixes in the same way now.
- With color management, every mix occurs before the output transform: dissolves, opacity, titles and subtitles.
- The timeline color space changes the mix. The output color space does not change it.
- With color management, the mix is not a straight line in the code values of a Rec.2100 ST2084 timeline. Its working space is not known.

## Decisions of the owner (2026-10-25)

- vidcut does what Resolve's color management does: every mix occurs in the timeline color space.
- The Auto of an export (D44) follows the timeline color space. An HDR timeline exports HDR where the codec carries HDR.
- Resolve's other color spaces come later, in [timeline-color-spaces-more.md](timeline-color-spaces-more.md).

## The plan

1. **A Timeline Color Space in the project** (`Settings`, `project.set`, the Project Settings window).
   - The choices: Auto, SDR (BT.709), HDR PQ and HDR HLG.
   - Auto is HDR when more than half of the timeline's pictures are HDR. This is the rule of D44 without the codec.
   - HDR is HLG when every HDR picture is HLG, else PQ.
2. **Every mix in the timeline color space.**
   - The mixes: dissolves, wipes, the opacity and edges of placed pictures, titles and subtitles.
   - Each picture of a mix goes to the timeline color space first.
   - The mix is a straight line in the timeline's code values. A dissolve uses the weights of D50. Resolve without color management mixes in the same way.
   - The mixed frame then goes to the output.
3. **The Auto of an export** (D44) is the timeline color space when the codec carries it with a 10-bit encoder on this computer. Else it is SDR.
4. **Exports.**
   - When the output has the timeline's colors, nothing changes.
   - When the output is different, only the frames with a mix change. A frame with one picture keeps the direct conversion, which gives the same result.
   - NVENC: `cuda::mix` and `cuda::burn` mix on the GPU. Then libplacebo converts the mixed frame on the GPU (AGENTS.md rule 9).
   - The other GPUs mix on the CPU ([gpu-dissolves.md](gpu-dissolves.md), [gpu-burn-in.md](gpu-burn-in.md)). There, the CPU also converts the mixed frame.
5. **The viewer.**
   - With HDR timeline colors, the layers, titles and subtitles go into the float picture of `Mixing` as HDR code values. One draw converts that picture for the screen.
   - With SDR timeline colors, the viewer draws as it does now.
6. **Picture renders** (`frame render`, the timeline's tiles and storyboards) go through `export::make::Maker`. Thus they follow the exports.
7. **Docs.**
   - X14: the timeline has a color space.
   - D44: Auto follows the timeline color space.
   - D50: the mix is in the timeline's code values.
   - A new deviation: Resolve's color management mixes in a working space that is not known. vidcut mixes in a straight line in the timeline's code values.
