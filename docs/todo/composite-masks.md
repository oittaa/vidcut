# Composite: masks and the rest

**When:** not decided.

vidcut has the composite modes from Normal to Darker Color (spec D75). These parts are still to do.

- **Foreground and the mask modes** (p. 1202):
  - The transparency of a clip at Foreground comes from the clip below it.
  - The lower clip is at Alpha, Inverted Alpha, Lum or Inverted Lum.
  - Measured in Resolve Studio 21.1.1: Lum is the luma of BT.709.
  - The mask m goes through a soft clamp c(m): 8m / 7 below 0.5, and (12m − 4m² − 1) / 7 above 0.5.
  - The inverted mask is c(1 − c(m)).
  - The measurements of these modes are not complete.
  - Why: a pair of clips mixes in a way that a plan of one layer at a time cannot give. The plan needs the lower clip's mask when it draws the upper clip.
  - Now: a Composite with these modes in an OTIO file stays a clip effect that does not play.
- **A title between layers:**
  - The export draws a title onto a frame in the code values of the frame. Thus it cuts the values of the layers below the title to the range of the code.
  - A layer with a mode above the title then mixes with the cut values. The viewer keeps them.
  - To do: draw a title onto the `Canvas` of the layers below it.
- **NVENC:** a frame with a composite mode is mixed on the CPU. A CUDA kernel can mix it on the GPU ([gpu-placement.md](gpu-placement.md)).
