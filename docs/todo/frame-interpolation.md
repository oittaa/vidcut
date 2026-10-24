# Frame interpolation

**When:** after Super Scale. The owner asks for it with an AI model where possible.

A clip that plays slower than its source, or at another frame rate than the timeline, shows the nearest source frame. Frame interpolation makes the frames between them.

## What Resolve has

- The Inspector of the Cut page has Retime Process: Nearest, Frame Blend and Optical Flow (pp. 739–740).
- With Optical Flow, Motion Estimation has Standard Faster, Standard Better, Enhanced Faster and Enhanced Better (p. 740).
- It also has Speed Warp Faster and Speed Warp Better, which use the DaVinci Neural Engine, and AI Speed Warp Metal on the Mac (p. 740).

## What is known

- Apple: `VTFrameRateConversionConfiguration` in VideoToolbox, from macOS 15.4.
- Intel: VPL 2.13 added AI frame interpolation.
- AMD and NVIDIA have their own. They are not checked yet.
- RIFE is a well-known open model for it. Its license and speed are not checked yet.
- It could run with the same engine as the AI upscalers ([ai-upscaling.md](ai-upscaling.md)).

## To do

1. Frame Blend, as Resolve has it.
2. Research RIFE and the GPU makers' interpolators: license, speed, and how many frames each needs.
3. A plan for Optical Flow and an AI mode, for the owner's OK.
