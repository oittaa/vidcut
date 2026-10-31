# Opacity of a compound clip

**When:** not decided.

- vidcut fades each picture and title inside a compound clip by the opacity and the soft edges of the compound clip (`place::Placing`).
- Where two pictures inside it overlap, the lower picture thus shows through the upper picture.
- A picture inside it at the Normal mode takes the composite mode of the compound clip.
- Resolve mixes the tracks of the compound clip first. Then it fades the mix by the opacity, or mixes it by the composite mode.
  - Measured in Resolve Studio 21.1.2, with 16-bit renders. Inside the compound clip, a picture cropped to its left half is over a full picture.
  - At an opacity of 50% over white, the frame is 0.5 × the mix + 0.5.
  - At Multiply over 50% gray, the frame is 0.5 × the mix.
  - The soft edges of a compound clip are not measured.
- vidcut blurs each picture inside a compound clip by the effects of the compound clip, in the frame (`Placing.effects`).
  - The blur thus comes after the transform and the crop of the compound clip.
  - A picture inside the compound clip is blurred by its own effects first.
  - Resolve's order for the effects of a compound clip is not measured.
- To do: mix the tracks of the compound clip first. Then fade the mix, or mix it by its composite mode, in the viewer and in the export.
- To do: measure where Resolve applies the effects of a compound clip.
