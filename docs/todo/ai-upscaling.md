# AI upscaling for Super Scale

**When:** next.

The exports of Super Scale have an AI upscaler: Real-ESRGAN on shaders of vidcut's own ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). This file holds what is known about the open models and about the ways to run them, and what is still to do.

## The model in vidcut

- **Model:** Real-ESRGAN `realesr-general-x4v3`, mixed with `realesr-general-wdn-x4v3`. Both are in release v0.2.5.0 of 2022-04-24 ([release](https://github.com/xinntao/Real-ESRGAN/releases/tag/v0.2.5.0)).
- **License:** the code is BSD-3-Clause. The weights have no license of their own. OpenModelDB lists them as BSD-3-Clause.
- **Training data:** DIV2K, Flickr2K and OST. DIV2K is "for academic research purpose only". It is not known if this binds the weights.
- It works on one frame at a time. Thus a seek or a cut needs no other frames. But a model that sees one frame alone can flicker.

## Other models

Each row is one version of a model. The numbers in this file for a model are of that version only.

| Model and version | Paper | Code license | Weights license |
|---|---|---|---|
| RealBasicVSR, official weights | [2111.12704](https://arxiv.org/abs/2111.12704), 2021-11-24 | Apache-2.0 | None stated. They hold SPyNet, which is non-commercial |
| BasicVSR++, official weights | [2104.13371](https://arxiv.org/abs/2104.13371), 2021-04-27 | Apache-2.0 | None stated. They hold SPyNet |
| FlashVSR+ Turbo | [2610.17552](https://arxiv.org/abs/2610.17552), 2026-10-13 | Apache-2.0 | Apache-2.0 |
| SeedVR2-3B | [2506.05301](https://arxiv.org/abs/2506.05301), 2025-06-05, ICLR 2026 | Apache-2.0 | Apache-2.0 |
| DOVE | [2505.16239](https://arxiv.org/abs/2505.16239), 2025-05-22, NeurIPS 2025 | Apache-2.0 | None stated. Its base, CogVideoX, has the CogVideoX License |
| SparkVSR | [2603.16864](https://arxiv.org/abs/2603.16864), 2026-03-17 | Apache-2.0 | The CogVideoX License |
| NanoVSR 644k | [2607.10495](https://arxiv.org/abs/2607.10495), 2026-07-11, ECCV 2026 | MIT | MIT |

- The paper dates, the code licenses and the licenses on the weights' model cards were checked on arXiv, GitHub and Hugging Face on 2026-10-24.
- The model card of SparkVSR says Apache-2.0, but the LICENSE file beside its weights is the CogVideoX License.
- The CogVideoX License allows academic research. Commercial use needs a registration, and the license can be revoked.
- The other facts come from the research notes of October 2026. They are not checked again.
- No paper compares any of these models with `realesr-general-x4v3`.

## The ways to run them

- **What vidcut has now:**
  - Voice isolation runs an ONNX model (DPDFNet) with `tract-onnx`, on the CPU. A network of Real-ESRGAN's size needs a GPU for video.
  - The `models` module downloads a model at its first use and checks it. An upscaler's weights can come the same way.
  - Transcription runs transcribe.cpp on ggml, on Vulkan or Metal, from a prebuilt library that vidcut downloads.
  - stable-diffusion.cpp is said to run ESRGAN on ggml. This is not checked yet.
- **Shaders of vidcut's own** on wgpu: vidcut uses these.
  - They need no new library. They run on every GPU maker that wgpu has.
  - Each network needs its own shaders. A small convolution network such as Real-ESRGAN's compact model needs few.
- **burn** on wgpu ([repo](https://github.com/tracel-ai/burn)):
  - MIT or Apache-2.0. It runs on every GPU maker.
  - It loads the official `.pth` files with `PytorchStore`. It needs no ONNX file and no Python.
  - It makes vidcut.exe 57 MB larger, because it brings its own kernel library (CubeCL).
  - Its fast path uses the tensor cores through Vulkan only (SPIR-V with cooperative matrices). Without it, it is 26 times slower.
- **ncnn** on Vulkan ([repo](https://github.com/Tencent/ncnn)):
  - BSD-3-Clause, 11.5 to 16.6 MB. It runs Real-ESRGAN on every GPU maker. A Mac needs MoltenVK.
  - Its Rust binding was last published in 2021. It cannot take vidcut's Vulkan device, so frames are copied or imported.
- **ONNX Runtime** through the `ort` crate:
  - MIT. Each system needs its own back end: DirectML on Windows, CoreML on the Mac.
  - DirectML has Microsoft's own license and is in sustained engineering. Its package is 193 MB.
  - The CUDA builds need more than 1 GB of CUDA libraries. The owner does not want such a download.
- **FFmpeg's `dnn_processing`:** FFmpeg 8.0 and 8.1 have no ONNX back end. FFmpeg 9.0 has one, but it copies the frames through memory.
  - Its ONNX back end takes one 4-D input with a batch of 1 (`dnn_backend_onnx.c`). Thus it runs one frame at a time, and no video model.
  - Its GPU devices are CUDA and DirectML. There is none for the Mac, or for AMD and Intel on Linux.
  - The BtbN build of FFmpeg 9.0.1 for Windows has no `dnn_processing` filter. Ubuntu 26.04 has FFmpeg 8.0.
  - Thus a feature only on FFmpeg 9.0 would reach few users.

## Video models in exports only

The owner wants the video models where they give better pictures. They can run in exports only:

- The viewer shows Super Scale only with View > Show Super Scale, and a proxy can play in its place. Thus a slow model does not hurt playback.
- An export reads each clip in order. Thus it can give the model the frames before and after each frame.
- At a cut, the frames past the cut usually are in the file. The export can read them.

## Notes on each model

- **RealBasicVSR, official weights:**
  - It looks both ways in time. Each output frame depends on every frame of its chunk.
  - Thus the export works in chunks, with extra frames on each side. A chunk starts again at a scene cut.
  - It has 6.3 M parameters. For each pixel, it needs about 5.4 times the work of `realesr-general-x4v3` (calculated from the code).
  - At the rate of the shaders now, that is about 430 ms for a 1280×534 frame. This is an estimate, not a measurement.
  - It needs new shader types: 7×7 convolutions, pooling, bilinear resize and a warp by optical flow.
  - Its official weight file, `RealBasicVSR.pth`, is 210 MB.
  - Its optical flow network, SPyNet, has a license for non-commercial research only. RealBasicVSR keeps SPyNet's weights frozen, so its weights hold them.
- **BasicVSR++, official weights:** they are for synthetic downscaling only, not for real footage. The model needs a deformable convolution.
- **FlashVSR+ Turbo** ([repo](https://github.com/OpenImagingLab/FlashVSR_Plus)):
  - It is a one-step diffusion model with a backbone of 1.3 B parameters, from Wan2.1, which is Apache-2.0.
  - It keeps the global attention in only 5 of its 30 layers. The other 25 layers use local attention.
  - It works on the frames in order. It keeps a cache of the attention of the earlier frames.
  - Its files are about 7.0 GB in 32-bit floats.
  - On an RTX 4090, it took 16 s for 89 frames at 1080p, 32 s at 2K and 90 s at 4K. These times are read from a figure.
  - 4K without tiles fits in 24 GB only when the attention cache moves to the CPU memory.
  - Its work per pixel is not calculated yet.
- **SeedVR2-3B** ([repo](https://github.com/ByteDance-Seed/SeedVR)):
  - It is a one-step diffusion transformer of 3.4 B parameters. It needs about 255 times the work of `realesr-general-x4v3` (calculated).
  - Its files are 14.6 GB in 32-bit floats.
  - Its optional color fix comes from another project, which bans commercial use. It is a short filter that vidcut can write itself.
- **NanoVSR 644k** ([repo](https://github.com/filippawlicki/nanovsr)):
  - It needs 0.68 times the work of `realesr-general-x4v3` (calculated).
  - It was trained only on clean downscaled video. Its model card warns against compressed sources.
- The official code of FlashVSR+, SeedVR2, DOVE and SparkVSR needs CUDA. vidcut would run them with its own shaders.

## Test results

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

Sources: the research of October 2026, with the links in this file and in the pull request that adds it.
