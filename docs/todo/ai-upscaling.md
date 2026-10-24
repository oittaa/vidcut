# AI upscaling for Super Scale

**When:** after Super Scale has its other GPUs ([super-scale.md](super-scale.md)). First a test of speed, with the owner's OK for the plan.

Super Scale uses classic upscalers ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). They make edges smooth, but they add no detail. A neural network can add detail. This file holds what is known about the open models and about the ways to run them.

## The models

- **Real-ESRGAN** ([repo](https://github.com/xinntao/Real-ESRGAN)):
  - The code is BSD-3-Clause. The weights have no license of their own. OpenModelDB lists them as BSD-3-Clause.
  - It was trained on DIV2K, Flickr2K and OST. DIV2K is "for academic research purpose only". It is not known if this binds the weights.
  - `realesr-general-x4v3` (4.9 MB) is for real footage. `realesr-animevideov3` (2.5 MB) is for animation. `RealESRGAN_x4plus` is 67 MB.
  - Speed, from the sources: `realesr-animevideov3` did 10 fps at 1080p on a V100 in PyTorch. A compact model did 17 fps with 1080p input on an RTX 4090 with TensorRT.
  - It works on one frame at a time. Thus a seek or a cut needs no other frames. But a GAN model on each frame alone can flicker.
  - Official ONNX and ncnn conversions exist. Real-ESRGAN-ncnn-vulkan is MIT.
- **SwinIR, HAT, DAT:**
  - The code is Apache-2.0. The weights have no license of their own.
  - They are slow: HAT took 725 ms for a 512×512 output on an RTX 4090. SwinIR took 0.54 s for a 256×256 input on an RTX 2080 Ti.
  - SwinIR-L was trained on FFHQ, which is CC BY-NC-SA. DAT has no official model for real footage.
  - In FP16, HAT, DAT and SwinIR can give NaN or black pictures on DirectML and CUDA ([rust_upscaler](https://github.com/kirinonakar/rust_upscaler)).
- **BasicVSR++, RealBasicVSR:**
  - The code is Apache-2.0. These are video models: each frame needs frames before and after it.
  - Thus a seek or a cut needs many frames around it. Their deformable convolution does not run on DirectML or natively on TensorRT.
  - No widely used ONNX or ncnn conversion was found.

## The ways to run them

- **What vidcut has now:**
  - Voice isolation runs an ONNX model (DPDFNet) with `tract-onnx`, on the CPU. A network of Real-ESRGAN's size needs a GPU for video.
  - The `models` module downloads a model at its first use and checks it. An upscaler's weights can come the same way.
  - Transcription runs transcribe.cpp on ggml, on Vulkan or Metal, from a prebuilt library that vidcut downloads.
  - stable-diffusion.cpp is said to run ESRGAN on ggml. This is not checked yet.
- **burn** on wgpu ([repo](https://github.com/tracel-ai/burn)):
  - MIT or Apache-2.0. It runs on every GPU maker, and it can share the wgpu device of the viewer.
  - `burn-onnx` turns the ONNX file into Rust code at build time. Thus each network is built into vidcut, and the weights load at run time.
  - No speed figures were found.
- **ncnn** on Vulkan ([repo](https://github.com/Tencent/ncnn)):
  - BSD-3-Clause, 11.5 to 16.6 MB. It runs Real-ESRGAN on every GPU maker. A Mac needs MoltenVK.
  - Its Rust binding was last published in 2021. It cannot take vidcut's Vulkan device, so frames are copied or imported.
- **ONNX Runtime** through the `ort` crate:
  - MIT. Each system needs its own back end: DirectML on Windows, CoreML on the Mac.
  - DirectML has Microsoft's own license and is in sustained engineering. Its package is 193 MB.
  - The CUDA builds need more than 1 GB of CUDA libraries. The owner does not want such a download.
- **FFmpeg's `dnn_processing`:** FFmpeg 8.0 and 8.1 have no ONNX back end. FFmpeg 9.0 has one, but it copies the frames through memory.

## Plan

1. A test: run `realesr-general-x4v3` with burn on wgpu, on the owner's RTX 4080. Measure the frames per second at 720p in and 1080p out.
2. If burn is too slow, the same test with ncnn on Vulkan.
3. Decide about the weights' license with the owner.
4. Then Super Scale gets an AI choice beside the classic upscaler.

Sources: the research of October 2026, with the links in this file and in the pull request that adds it.
