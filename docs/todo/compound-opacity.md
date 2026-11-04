# Soft edges and effects of a compound clip

**When:** not decided.

- vidcut fades each picture and title inside a compound clip by the soft edges of the compound clip (`place::Placing`).
  - The soft edges of a compound clip are not measured.
- vidcut blurs each picture inside a compound clip by the effects of the compound clip, in the frame (`Placing.effects`).
  - The blur thus comes after the transform and the crop of the compound clip.
  - A picture inside the compound clip is blurred by its own effects first.
  - Resolve's order for the effects of a compound clip is not measured.
- A picture with a composite mode that has no picture of its compound clip below it mixes with the tracks below the compound clip.
  - Resolve's rule for this picture is not measured.
- To do: measure the soft edges of a compound clip.
- To do: measure where Resolve applies the effects of a compound clip.
- To do: measure what a composite mode mixes with where no picture of its compound clip is below it.
