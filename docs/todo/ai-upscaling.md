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

AI Best against the official PyTorch code, on 2026-10-24: 33 frames of a 128×64 copy of the 480×256 clip, enlarged 4 times to 512×256.

- vidcut's bicubic input is bit-identical to PIL's in all 33 frames.
- Against the official code in 32-bit floats, the frames are at 59.5 dB PSNR on average, and 52.1 dB at the least.
- The official code in 16-bit floats is at 56.5 dB from its own 32-bit frames. It has the same low point, at frames 13 to 16.
- The largest value that vidcut keeps as a 16-bit float is 174 in this clip.

## Decisions

- The weights download at their first use. vidcut does not ship them.
- The AI model is NanoVSR 5.4M. In side-by-side tests, the owner ranked it above Real-ESRGAN's compact model.
- AI has no Noise Reduction. NanoVSR has no such control.
- AI has no size limit. Its memory grows with the size of the source picture.
- Users have different needs. Some wait a long time for the best pictures, and others want good pictures fast.
  - Thus Super Scale can offer more than one AI model: a fast default, and slower models with better pictures.
  - Speed alone does not rule out a model. A slow model must give clearly better pictures.
  - FlashVSR+ Turbo ([repo](https://github.com/OpenImagingLab/FlashVSR_Plus)) is the slow model, AI Best. NanoVSR 5.4M is the fast one, AI Fast.
  - AI Best runs in the viewer too, as AI Fast does, however slow it is.

## Plan

These parts are still to do:

1. AI Best (FlashVSR+ Turbo). Its engine and its place in Super Scale are in the code.
   1. The check against the official PyTorch output in bf16 with the Triton kernel, at 1920×1024, with `VIDCUT_FLASHVSR_DUMP` ([tools](../tools.md)).
   2. A run at 3840×2176, where most KV caches stay in main memory.
   3. Speed: measure, then the tensor cores through wgpu's cooperative matrices, on Vulkan and Metal.
2. A test of the chunk edges: each chunk starts from zero. A carried forward state or overlapping chunks may hide a change every 15 frames.
3. Speed. PyTorch in 16-bit floats is faster on small pictures. The gain of each step is not known until it is measured.
   1. Measure where the time goes, with GPU timestamps: the network, the copies, the decoding and the encoding.
   2. Use the tensor cores. PyTorch's convolutions use them through cuDNN; vidcut's shaders do not. wgpu's cooperative matrices take only 8×8 32-bit floats now.
   3. Winograd convolutions: 16 multiplications for each 2×2 block of output of a 3×3 convolution, in place of 36.
   4. 16-bit arithmetic in the shaders (`shader-f16`), on GPUs that run it faster than 32-bit.
   5. Join the add of the features into the first convolution of each chain. This saves a pass over the whole frame for each frame and direction.
   6. The 1×1 join reads its weights from storage memory for each pixel. It can use workgroup memory, as the 3×3 convolutions do.
   7. Run all tiles of a frame in one submission. Each tile now waits for its own copy back to memory.
   8. Make the bind groups once for each chunk, not for each pass.
   9. Decode each frame once. The chunk reader decodes the frames a second time.
   10. Keep the frames on the GPU, from the decoder through the shaders to the encoder. They go through memory now. The viewer and the export both need it.
   11. Let the CPU decode and encode while the GPU runs the network.
4. The shaders on the GPU that `gpu.device` names. They use the GPU that wgpu prefers now.

The weights are on NanoVSR's release v1.0:

| File | Bytes | SHA-256 |
|---|---|---|
| `nanovsr_5.4m.pth` | 24624333 | `42a53e1dc9b928947ea9cea85d7dd6f7bd82b325e92374dc1946d2cbcadf31bc` |

The weights of AI Best are on Hugging Face (`ruchangyao/FlashVSR_plus`, revision `46b79520946ac67dec94d1566490690953c7debc`) and in the code repo (`OpenImagingLab/FlashVSR_Plus`, commit `b548a2cc2c929d3d4676f2973abe27876ec4dee7`):

| File | Bytes | SHA-256 |
|---|---|---|
| `turbo/dit.safetensors` | 5676070392 | `7862b92178176fa3aebc34c6c071daca6b56ab1dddb8e7ff9a01c47f5226ec8b` |
| `turbo/lq_proj_in.safetensors` | 1151384272 | `e77b378ae12342eefbb58f924256f362a151b93ce73dd32808fcdaca9c051496` |
| `TCDecoder.ckpt` | 189018333 | `e224bdcf2f52745cbf4d393ff5374c2ba09e90285d5d19062d2bf63b915b6161` |
| `prompt_tensor/posi_prompt.pth` | 4195504 | `4601107a11e4e11a936a6b79df579e54dbc99872132bf542151f0ffd65b4b1ef` |
