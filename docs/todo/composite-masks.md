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
- **Values between layers:**
  - Resolve keeps values above 1 and below 0 between layers.
  - The export keeps each layer in the code values of the frame. The viewer keeps them in 8-bit RGB, or in 16-bit floats for an HDR timeline.
  - Thus a stack of layers with modes can give other values than Resolve.
  - To do: mix the layers of a frame with a mode in floats.
- **NVENC:** a frame with a composite mode is mixed on the CPU. A CUDA kernel can mix it on the GPU ([gpu-placement.md](gpu-placement.md)).
