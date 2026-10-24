# AI upscaling for Super Scale

**When:** next.

Super Scale has an AI upscaler: Real-ESRGAN on shaders of vidcut's own ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). This file holds its measurements, the decisions and what is still to do.

## The model in vidcut

- **Model:** Real-ESRGAN `realesr-general-x4v3`, mixed with `realesr-general-wdn-x4v3`. Both are in release v0.2.5.0 of 2022-04-24 ([release](https://github.com/xinntao/Real-ESRGAN/releases/tag/v0.2.5.0)).
- **License:** the code is BSD-3-Clause. The weights have no license of their own. OpenModelDB lists them as BSD-3-Clause.
- **Training data:** DIV2K, Flickr2K and OST. DIV2K is "for academic research purpose only". It is not known if this binds the weights.
- It works on one frame at a time. Thus a seek or a cut needs no other frames. But a model that sees one frame alone can flicker.

## Measurements

The tests ran on the owner's RTX 4080 SUPER on 2026-10-24, in programs outside vidcut. They are the only measurements in this file.

- The input was one 1280×534 frame of "Tears of Steel". The output was 5120×2136.

| Engine | Model | Precision | Time for each frame |
|---|---|---|---|
| burn 0.22.1, Vulkan | `realesr-general-x4v3` | 32-bit | 1.08–1.14 s |
| burn 0.22.1, Vulkan | `realesr-general-x4v3` | 16-bit | 39 ms (25 fps) |
| burn 0.22.1, Vulkan | `realesr-general-x4v3`, 640×267 input | 16-bit | 8.5 ms |
| burn 0.22.1, without Vulkan | `realesr-general-x4v3` | 16-bit | 1027 ms |
| Own WGSL shaders | `realesr-general-x4v3` | 16-bit values, 32-bit sums | 80 ms (12.5 fps) |

- The 16-bit pictures of burn are at most 2 of 255 levels from the 32-bit ones.
- The pictures of the own shaders are at most 3 of 255 levels from those of burn.
- The speed of the own shaders inside vidcut is not measured yet. vidcut adds the tiles and the copies through memory.
- The pictures are much sharper than a classic upscaler. The model removes compression blocks. It also makes skin smooth, as if painted.
- The official denoise strength fixes most of this. It mixes the weights of `realesr-general-x4v3` (times the strength) and `realesr-general-wdn-x4v3` (times one less the strength) once, at load. The speed is the same.
- At a strength of 0.5, the edges stay sharp and the skin keeps its lines. The owner and Claude both found 0.5 the best.

## Decisions

- The weights download at their first use. vidcut does not ship them. The owner accepts their license on that basis.
- Super Scale gets Noise Reduction, as Resolve has (p. 433). Low, Medium and High give a denoise strength of 0.2, 0.5 and 1.0. Medium is the default.
- A mix with the classic upscaler can come later, as an extra.
- Users have different needs. Some wait a long time for the best pictures, and others want good pictures fast.
  - Thus Super Scale can offer more than one AI model: a fast default, and slower models with better pictures.
  - Speed alone does not rule out a model. A slow model must give clearly better pictures.

## Plan

AI Super Scale is built for exports and for the viewer ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). These parts are still to do:

1. Winograd convolutions in the shaders. They need 16 multiplications for each 2×2 block of output, in place of 36. burn's tensor cores were 2 times faster than the shaders are now.
2. Frames from the GPU decoders straight into the shaders, with no copy through memory. The viewer and the export both need it.
3. The shaders on the GPU that `gpu.device` names. They use the GPU that wgpu prefers now.
4. A mix with the classic upscaler, as an extra.
5. A decision of the owner on the SPyNet license of RealBasicVSR. Then, with the owner's OK, a side-by-side test against Real-ESRGAN on the same clip.
6. A side-by-side test on the owner's clips: FlashVSR+ Turbo and SeedVR2-3B against `realesr-general-x4v3`. It needs the owner's OK and a measurement plan. The winner can become the slow tier.

The weights are on Real-ESRGAN's release v0.2.5.0:

| File | Bytes | SHA-256 |
|---|---|---|
| `realesr-general-x4v3.pth` | 4885111 | `8dc7edb9ac80ccdc30c3a5dca6616509367f05fbc184ad95b731f05bece96292` |
| `realesr-general-wdn-x4v3.pth` | 4885111 | `1641f8c4464b9f097c9fdda5589273713f67cf59f3d909e0bd688f0cee269dca` |
