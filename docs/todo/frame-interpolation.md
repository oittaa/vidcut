# Frame interpolation

**When:** after Super Scale. The owner asks for it with an AI model where possible.

A clip that plays slower than its source, or at another frame rate than the timeline, shows the nearest source frame. Frame interpolation makes the frames between them.

## What Resolve has

- The Inspector of the Cut page has Retime Process: Nearest, Frame Blend and Optical Flow (pp. 739–740).
- With Optical Flow, Motion Estimation has Standard Faster, Standard Better, Enhanced Faster and Enhanced Better (p. 740).
- It also has Speed Warp Faster and Speed Warp Better, which use the DaVinci Neural Engine, and AI Speed Warp Metal on the Mac (p. 740).

## To do

1. Frame Blend, as Resolve has it: a shader that mixes two frames.
2. An AI mode (Speed Warp) with RIFE 4.25 as shaders of vidcut's own, with a scene-cut check. A plan for the owner's OK comes first.
3. A plan for Optical Flow, for the owner's OK: a classic block matcher in WGSL, or RIFE at a lower resolution.
4. The GPU makers' interpolators, as extras later.
