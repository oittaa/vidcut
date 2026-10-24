# AI upscaling for Super Scale

**When:** next.

Super Scale has an AI upscaler: NanoVSR 5.4M on shaders of vidcut's own ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). This file holds its measurements, the decisions and what is still to do.

## The model in vidcut

- **Model:** NanoVSR 5.4M, release v1.0 of 2026-07-11 ([repo](https://github.com/filippawlicki/nanovsr), [paper](https://arxiv.org/abs/2607.10495)).
- **License:** MIT for the code and the weights.
- **Training data:** REDS and Vimeo-90K, downscaled without noise or compression. Its model card warns that it may not suit heavily compressed sources.
- It looks both ways in time. Thus it reads the frames in chunks of 15, as the official code does.

## Measurements

The tests ran on the owner's RTX 4080 SUPER (16 GB) on 2026-10-24. NanoVSR 5.4M enlarged each clip 4 times.

| Clip | vidcut | PyTorch, 32-bit | PyTorch, 16-bit |
|---|---|---|---|
| 480×256, 89 frames | 8.0 s, 1.6 GB | 8.9 s, 4.0 GB | 4.6 s, 2.4 GB |
| 1280×534, 30 frames | 14.5 s, 3.9 GB | 158.7 s, 23.0 GB | 12.6 s, 12.9 GB |
| 1920×1080, 30 frames | 40.3 s, 10.1 GB | not done in 5 minutes | not done in 5 minutes |

- vidcut's time is the whole export: decoding, the network, the fitting and the encoding.
- vidcut's memory is the peak of the whole card less the 1.0 GB that it used before.
- PyTorch's time includes loading the model and reading and writing the video. Its memory is what PyTorch reserved. 23.0 GB does not fit on the card, so the driver used main memory.
- vidcut's pictures are within 2 of 255 levels of PyTorch's 32-bit pictures. The PSNR between them is 68 dB.

## Decisions

- The weights download at their first use. vidcut does not ship them.
- The AI model is NanoVSR 5.4M. In side-by-side tests, the owner ranked it above Real-ESRGAN's compact model.
- AI has no Noise Reduction. NanoVSR has no such control.
- AI has no size limit. Its memory grows with the size of the source picture.
- Users have different needs. Some wait a long time for the best pictures, and others want good pictures fast.
  - Thus Super Scale can offer more than one AI model: a fast default, and slower models with better pictures.
  - Speed alone does not rule out a model. A slow model must give clearly better pictures.
  - FlashVSR+ Turbo ([repo](https://github.com/OpenImagingLab/FlashVSR_Plus)) is the choice for the slow model.

## Plan

These parts are still to do:

1. FlashVSR+ Turbo as the slow model. It needs a transformer engine on wgpu: matrix multiplies, attention and a cache of earlier frames.
2. A test of the chunk edges: each chunk starts from zero. A carried forward state or overlapping chunks may hide a change every 15 frames.
3. Speed: the 1×1 join and the copies of the tiles are simple shaders now.
4. Frames from the GPU decoders straight into the shaders, with no copy through memory. The viewer and the export both need it.
5. The shaders on the GPU that `gpu.device` names. They use the GPU that wgpu prefers now.

The weights are on NanoVSR's release v1.0:

| File | Bytes | SHA-256 |
|---|---|---|
| `nanovsr_5.4m.pth` | 24624333 | `42a53e1dc9b928947ea9cea85d7dd6f7bd82b325e92374dc1946d2cbcadf31bc` |
