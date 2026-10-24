# Frame interpolation

**When:** after Super Scale. The owner asks for it with an AI model where possible.

A clip that plays slower than its source, or at another frame rate than the timeline, shows the nearest source frame. Frame interpolation makes the frames between them.

## What Resolve has

- The Inspector of the Cut page has Retime Process: Nearest, Frame Blend and Optical Flow (pp. 739–740).
- With Optical Flow, Motion Estimation has Standard Faster, Standard Better, Enhanced Faster and Enhanced Better (p. 740).
- It also has Speed Warp Faster and Speed Warp Better, which use the DaVinci Neural Engine, and AI Speed Warp Metal on the Mac (p. 740).

## What is known

- **RIFE 4.25** ([Practical-RIFE](https://github.com/hzwer/Practical-RIFE)):
  - Its code and its weights are MIT. The project recommends 4.25 as the default.
  - It takes two frames and a time from 0 to 1. Thus a retime at any speed needs only the two frames around each new frame.
  - Its file is 24.6 MB. [vs-rife](https://github.com/HolyWu/vs-rife) (MIT) has the files on GitHub releases. Thus they can download at the first use, checked by SHA-256.
  - It needs only simple shader types: 3×3 convolutions, a transposed convolution, bilinear resize and a backward warp.
  - The warp needs 32-bit floats.
  - It does not find scene cuts. Thus vidcut needs its own check, so that no frames morph across a cut.
  - No speed figure for 4.25 was found. Version 4.6 did 148 fps at 1080p on an RTX 4090 with TensorRT.
- **Other models:**
  - IFRNet has MIT code. The license of its weights is not stated.
  - FILM (Apache-2.0) makes only the middle frame, and it is slow. EMA-VFI (Apache-2.0) uses attention.
  - GIMM-VFI and AMT are for non-commercial use only.
- **The GPU makers' interpolators:**
  - Apple: `VTFrameRateConversionParameters` in VideoToolbox, from macOS 15.4. It is an Objective-C API.
  - Intel: VPL API 2.12 added AI frame interpolation (`mfxExtVPPAIFrameInterpolation`). Which GPUs have it is not checked.
  - AMD: AMF FRC, on Windows. It only doubles the frame rate. FFmpeg 9.0 has it as `frc_amf`.
  - NVIDIA: the FRUC library of the Optical Flow SDK, from Turing GPUs. Whether an app can ship it is not checked.
  - Each covers one maker or one system, and they do not behave the same.
- **FFmpeg's filters:** `minterpolate` runs on the CPU, with block matching. `framerate` blends two frames, as Frame Blend does. No GPU filter does it.
- It can run with the same kind of shaders as the AI upscaler ([ai-upscaling.md](ai-upscaling.md)).

## To do

1. Frame Blend, as Resolve has it: a shader that mixes two frames.
2. An AI mode (Speed Warp) with RIFE 4.25 as shaders of vidcut's own, with a scene-cut check. A plan for the owner's OK comes first.
3. A plan for Optical Flow, for the owner's OK: a classic block matcher in WGSL, or RIFE at a lower resolution.
4. The GPU makers' interpolators, as extras later.

Sources: the research of October 2026, with the links in this file.
