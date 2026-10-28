# AI upscaling for Super Scale

**When:** next.

Super Scale has two AI upscalers on shaders of vidcut's own: NanoVSR 5.4M (AI Fast) and FlashVSR+ Turbo (AI Best) ([X91](../deviations.md#x91-super-scale-uses-the-upscaler-of-the-gpu)). This file holds their measurements, the decisions and the open work.

## The model in vidcut

- **Model:** NanoVSR 5.4M, release v1.0 of 2026-07-11 ([repo](https://github.com/filippawlicki/nanovsr), [paper](https://arxiv.org/abs/2607.10495)).
- **License:** MIT for the code and the weights.
- **Training data:** REDS and Vimeo-90K, downscaled without noise or compression. Its model card warns that it may not suit heavily compressed sources.
- It looks both ways in time. Thus it reads the frames in chunks of 15, as the official code does.

## Measurements

The tests ran on the owner's RTX 4080 SUPER (16 GB) on 2026-10-24. NanoVSR 5.4M enlarged each clip 4 times.

| Clip | vidcut | PyTorch, 32-bit | PyTorch, 16-bit |
|---|---|---|---|
| 480×256, 89 frames | 4.3 s, 1.4 GB | 8.9 s, 4.0 GB | 4.6 s, 2.4 GB |
| 1280×534, 30 frames | 7.1 s, 3.9 GB | 158.7 s, 23.0 GB | 12.6 s, 12.9 GB |
| 1920×1080, 30 frames | 20.2 s, 10.1 GB | not done in 5 minutes | not done in 5 minutes |

- vidcut's time is the time that the export reports. It is the whole export: decoding, the network, the fitting and the encoding.
- vidcut's memory is the peak of the whole card less the 1.0 GB that it used before.
- PyTorch's time includes loading the model and reading and writing the video. Its memory is what PyTorch reserved. 23.0 GB does not fit on the card, so the driver used main memory.
- vidcut's pictures are within 2 of 255 levels of PyTorch's 32-bit pictures. The PSNR between them is 68 dB.

AI Best against the official PyTorch code, on 2026-10-24: 33 frames of a 128×64 copy of the 480×256 clip, enlarged 4 times to 512×256.

- vidcut's bicubic input is bit-identical to PIL's in all 33 frames.
- Against the official code in 32-bit floats, the frames are at 59.5 dB PSNR on average, and 52.1 dB at the least.
- The official code in 16-bit floats is at 56.5 dB from its own 32-bit frames. It has the same low point, at frames 13 to 16.
- The largest value that vidcut keeps as a 16-bit float is 174 in this clip.
- The official code in bf16 is at 43.6 dB from its own 32-bit frames, with either attention kernel. Its latents are 2.6 to 2.8% from the 32-bit latents.

AI Best against the official code in bf16 with its Triton kernel, at 1920×1024 (the 89 frames of the 480×256 clip, 4 times):

- The frames are at 45.4 dB PSNR on average, and 42.7 dB at the least. The latents are 2.6 to 2.9% from the official ones.
- Thus vidcut is as far from the bf16 frames as the bf16 frames are from 32-bit floats.
- A 32-bit run of the official code does not fit at this size: its dense attention masks need about 28 GB.

AI Best speed and memory on the owner's RTX 4080 SUPER (16 GB), on 2026-10-24:

- Each vidcut run is a CLI export: `project create --fps F --width W --height H`, `media add`, `edit append m1 --range 0f..Nf`, `media set m1 --super-scale true --upscaler ai_best`, `export start out.mkv`.
- The memory is the peak of the whole card, from `nvidia-smi` every 0.5 s, less the memory in use before.

| Clip | vidcut | Official code, bf16, Triton |
|---|---|---|
| 480×256 to 1920×1024, 89 frames | 86.8 s (78.5 s for the export after the model loads); 11,017 MiB | 54.4 s; 11,047 MiB |
| 1920×1080 to 3840×2176, 30 frames | 311.9 s (303.5 s for the export); 14,748 MiB; 16.6 GiB of main memory | not done after 15 minutes, when a time limit stopped it; 15,407 MiB |

- The official time includes loading the model and compiling its Triton kernel. The official memory comes from `nvidia-smi` every 200 ms.
- At 4K the official code kept the KV caches of its 5 global blocks in main memory.
- At 4K, 30 frames take 3 chunks, so the DiT runs on 48 frames.

AI Best and AI Fast memory and time on the owner's RTX 4080 SUPER (16 GB), on 2026-10-28:

- Each run is a CLI export with `VIDCUT_DEBUG=1`. 13 frames are one chunk of AI Best, and 29 frames are two chunks.
- The GPU memory is the dedicated memory of the vidcut process, from the "GPU Process Memory" counters every 100 ms.
- The weights and the plan are the memory that vidcut lends for AI work, from its debug lines. AI Fast lends one amount.
- The RAM is the peak working set. The busy shaders come from `nvidia-smi dmon`, every second.

| Run | Export | Process | CPU | GPU peak | Weights + plan | Peak less these | RAM | Shaders busy |
|---|---|---|---|---|---|---|---|---|
| AI Best, 480×256 to 1920×1024, 13 frames | 10.6 s | 17.8 s | 11.6 s | 10,073 MB | 3,181 + 5,494 MB | +1,398 MB | 2,951 MB | 55% |
| AI Best, 1920×1080 to 3840×2160, 13 frames | 47.9 s | 55.7 s | 28.5 s | 13,556 MB | 3,181 + 10,429 MB | −54 MB | 14,768 MB | 85% |
| AI Best, 1920×1080 to 3840×2160, 29 frames | 129.9 s | 137.0 s | 60.8 s | 13,557 MB | 3,181 + 10,429 MB | −53 MB | 18,128 MB | 93% |
| AI Fast, 480×256 to 1920×1024, 13 frames | 0.7 s | 1.3 s | 0.8 s | 1,286 MB | 638 MB | +648 MB | 564 MB | not measured |
| AI Fast, 1920×1080 to 3840×2160, 13 frames | 9.2 s | 9.9 s | 0.8 s | 11,376 MB | 8,877 MB | +2,499 MB | 1,349 MB | 73% |

- A second run of each AI Best size gave the same peak and export time.
- AI Fast at 480×256 ran for 1.3 s. That is too short for the busy shaders, which `nvidia-smi` gives once a second.
- At 4K, AI Best's peak equals the weights and the plan. The GPU memory stays the same from chunk to chunk.
- AI Best's caches at 4K:
  - Chunk 1 keeps 29 of its 30 caches in main memory. It waits 25.6 s of its 47.6 s for the copies back.
  - In the 29-frame run, chunk 2 had 3.2 GB of the GPU free and needed 3.9 GB for one cache. Thus all 30 caches went to main memory.
  - Chunk 2 waited 46.3 s of its 75.0 s, and copied 28.1 GB there and back.
  - vidcut lent 14.2 GB of main memory for chunk 1, and 15.3 GB for chunk 2.
- After AI Best loads its model, the process holds 4.2 GB of the GPU. The load peaks at 5.6 GB. vidcut counts 3.2 GB for the weights.
- At 4K, the GPU memory of the process grows by 1.7 GB just after the decoder of the 1080p source starts. This occurs with AI Best and AI Fast.
- The shared GPU memory reaches 1.1 GB at the same time. With AI Best at 4K, it grows up to 1.2 GB more while the blocks run.

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

These parts are open:

1. AI Best (FlashVSR+ Turbo). Its engine and its place in Super Scale are in the code.
   1. The caches in main memory. At 4K, the waits for their copies take 54% of chunk 1 and 62% of chunk 2.
      - Find what fills the GPU before the plan: the load leaves 1 GB more than vidcut counts, and 1.7 GB comes with the 1080p decoder.
      - Then keep more caches on the GPU.
   2. Speed: find where the time goes, then use the tensor cores through wgpu's cooperative matrices, on Vulkan and Metal.
2. A test of the chunk edges: each chunk starts from zero. A carried forward state or overlapping chunks may hide a change every 15 frames.
3. AI Fast's memory plan. At 4K, the peak is 2.5 GB over the plan. The 1.7 GB that comes with the decoder is part of this.
4. Speed. The gain of each step is not known until it is measured.
   1. Measure where the time goes, with GPU timestamps: the network, the copies, the decoding and the encoding.
   2. Use the tensor cores. PyTorch's convolutions use them through cuDNN; vidcut's shaders do not. wgpu's cooperative matrices take only 8×8 32-bit floats.
   3. More sums in each thread of the Winograd shader. Reads of workgroup memory limit its main loop.
   4. 16-bit arithmetic in the shaders (`shader-f16`), on GPUs that run it faster than 32-bit.
   5. Make the bind groups once for each chunk, not for each pass.
   6. Decode each frame once. The chunk reader decodes the frames a second time.
   7. Keep the frames on the GPU, from the decoder through the shaders to the encoder. They go through memory. The viewer and the export both need it.
   8. Let the CPU decode and encode while the GPU runs the network.

The files of both models, with their sources, sizes and SHA-256, are in `crates/vidcut-core/src/catalog.rs`.
