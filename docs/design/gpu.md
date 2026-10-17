# GPU

vidcut decodes, scales, mixes, shows, and encodes on the GPU when the GPU gives the same results as the CPU.
One GPU does all of those steps.
A decoded frame stays in GPU memory while the next step can take it there.
The frame comes down to memory when the next step cannot.
[playback](playback.md) and [export](export.md) cover the pipelines around this page.
Driver quirks are in [gotchas](../gotchas.md).

```mermaid
flowchart LR
  dec["hw::GpuDecoder<br>decodes on the GPU"] --> frame["frame on a route::Place<br>Cuda, Vaapi, D3d11, VideoToolbox, Vulkan"]
  frame -->|"Route::Kept, hw::Keeper"| store["player::store::Store<br>video memory budget"]
  store -->|"imported or copied on the GPU"| shown["viewer textures<br>gui/picture.rs"]
  frame -->|"Route::Gpu"| fitted["fitted on the GPU<br>CudaPictures, hw::Fitter"]
  fitted -->|"CUDA only"| mixed["dissolve mixed<br>cuda::mix"]
  fitted --> genc["GPU encoder<br>on the same device"]
  mixed --> genc
  frame -->|"Route::Shrunk, Route::Whole"| mem["memory<br>hw::Shrinker, hw::download"]
  store -->|"budget, failed import"| mem
  mem --> cpu["Normalizer, export::mix<br>CPU encoders"]
  mem -->|"uploaded"| genc
```

## Choosing the GPU and the decoder

`gpus::list` lists the computer GPUs once.
Each GPU is named by its PCI ids and its slot (`Gpu::id`).
The entry also has its CUDA device, its Direct3D 11 adapter, or its render node.
`gpus::in_use` is the GPU that `gpu.device` names (`gpus::choose`).
When `gpu.device` names none, `gpus::automatic` chooses.
It chooses the first NVIDIA CUDA device.
Otherwise it chooses the first other GPU.
A Mac lists no GPU.

`GpuDecoder::best` opens the decoder on the GPU in use, once for each GPU.
On NVIDIA, the decoder is NVDEC, with Vulkan beside it for ProRes.
On other Windows GPUs, the decoder is Direct3D 11.
On Linux, the decoder is VA-API.
On a Mac, the decoder is VideoToolbox.
`VIDCUT_HWDEC` overrides the choice for tests.
The player decodes on that decoder.
Exports, proxies, renders, and the timeline tiles use `GpuDecoder::own`.
`GpuDecoder::own` is NVDEC on its own CUDA stream.
The other decoders use their own device on the same GPU.

Those jobs do not use the player decoder.
On the player decoder, their work would wait in line with playback.

`gpu.acceleration` is on by default.
It lets the GPU decode, scale, and encode.
When it is off, the CPU does every step, even for a preset that names a GPU encoder.
`gpu.decoding` is on by default.
It lets the GPU decode.
When it is off, the CPU decodes and scales, and the GPU still encodes.
The app reads both flags through `preferences::gpu_decoding`.

`GpuDecoder::kind_for` picks the decoder kind for a stream.
`decode::gpu_decoder` opens the FFmpeg decoder with a hardware configuration for that device.
It does not open a wrapper of a decoder that the GPU owns.
`GpuDecoder::why_not` names what is left to the CPU.
A stream that the GPU decoder refuses is decoded on the CPU (`VideoReader::open_gpu_or_cpu`).
Stills and image sequences decode on the CPU.

## Where a frame lives

`route::Place` is the place of a picture.
The place is `Memory`, or one of the five GPU places of `hw::Kind`, by pixel format.
`route::Facts` holds what the planner knows about a frame that is bound for a `route::Dest`.
A destination is memory, an encoder, or the viewer.
The facts say whether the GPU fits the frame for that encoder (`hw::can_fit`).
They say whether the GPU hands pictures to the encoder place (`hw::reaches`).
CUDA and Vulkan hand pictures to each other.
They say whether the GPU mixes a dissolve (`hw::can_mix`).
Only CUDA mixes a dissolve.
They also record what failed before.
`route::plan` is a pure function.
It returns the `Route`.

| `Route` | What happens to the picture |
|---|---|
| `Route::Gpu` | The GPU fits the picture and hands it on in GPU memory. |
| `Route::Kept` | The picture is kept as decoded, for the viewer. |
| `Route::Shrunk` | The GPU scales the picture down, copies it down, and the CPU finishes it. |
| `Route::Whole` | The picture is copied down whole. |
| `Route::Cpu` | The picture is already in memory. |

A GPU route wins over a CPU route.
A route that fails becomes a fact.
The frame, and later frames of its file, are planned again without that route.

## Each decoder's frames

In playback:

| Decoder | Where it runs | Codecs | Kept in the player | Shown by the viewer |
|---|---|---|---|---|
| NVDEC (CUDA) | NVIDIA, on Linux and on Windows | The CUDA decoders in FFmpeg. Not ProRes. | `cuda::keep` copies the picture into memory of its own. | CUDA copies it into a buffer shared with wgpu (`SharedBuffer`). |
| Vulkan | NVIDIA, beside NVDEC | ProRes, where FFmpeg decodes it on Vulkan. | `vulkan::keep` keeps the picture as decoded. | The images are imported into wgpu. |
| VA-API | Linux, not on NVIDIA | The VA-API decoders in FFmpeg. | `vaapi::Keeper` uses `vaCopy` into surfaces of its own. | The surfaces are exported as DMA-BUFs and imported. |
| Direct3D 11 | Windows, not on NVIDIA | The D3D11VA decoders in FFmpeg. On AMD, not H.264 with open GOPs. | `d3d11::Keeper` copies the picture into shareable textures. | The shared textures are imported. |
| VideoToolbox | macOS | The VideoToolbox decoders in FFmpeg. Not H.264 with open GOPs. | The picture is kept as decoded. | IOSurface planes are used as Metal textures. |

vidcut learns three facts about a VA-API device.
`NO_COPY` means the driver cannot copy surfaces.
`WAYS` names the ways its frames come down.
`SIZES` is the size of its surfaces.
The facts live in `crates/vidcut-av/src/hw/vaapi.rs`.
vidcut keeps them by the address of the VA display, or by the address of the device.
It keeps no reference to either object.
A device that opens where a freed device was, including on another GPU, inherits those facts.

In exports:

| Decoder | Fitted on the GPU for | Fit filters | Dissolves | Comes down for |
|---|---|---|---|---|
| NVDEC (CUDA) | NVENC, and the Vulkan ProRes encoder for proxies. | `CudaPictures`: `scale_cuda`, `pad_cuda`, and libplacebo for turns, colors, and HDR. | `cuda::mix` | Every other encoder. |
| Vulkan | NVENC, after a copy into CUDA on the GPU. Also the Vulkan ProRes encoder. | libplacebo (`CudaPictures`) | `cuda::mix` | Every other encoder, and a picture with alpha. |
| VA-API | The VAAPI encoder on its device. | `transpose_vaapi`, `scale_vaapi`, and `pad_vaapi`. Colors stay unchanged. | In memory. | Another encoder, including QSV. A color change. A dissolve. |
| Direct3D 11 | AMF on its adapter. | The video processor. Colors stay unchanged. An 8-bit picture is not fitted into a 10-bit picture. | In memory. | Another encoder, including QSV. A color change. A dissolve. |
| VideoToolbox | Its own encoders. | `transpose_vt` and `scale_vt`, when the colors and the layout stay. | In memory. | Padding, a layout change, a color change, or a dissolve. |

When the driver will not take the `cuda::mix` kernel, the export mixes later dissolves on the CPU.
The export result says why.
When a frame comes down and the output shows it smaller, the decoder GPU shrinks it first.
The shrink filters are `scale_cuda`, `scale_vulkan`, `scale_vaapi`, the video processor, and `scale_vt`.
The viewer blends the layers of a dissolve wherever each layer is.
Any decoder's frames come down for a CPU encoder, for a render, and for tiles.
In the player, a frame comes down in four cases.
The viewer cannot show that kind (`Gpu::gpu_pictures`).
An import fails.
Pictures are shown shrunk.
The pool leaves the video memory budget.
A picture made in memory goes up into the frames of a GPU encoder (`Upload`, `CudaFrames::upload`).

## Encoders

| Encoder | GPUs | Codecs |
|---|---|---|
| NVENC | NVIDIA | H.264, HEVC, and AV1. |
| VAAPI | Linux, not NVIDIA | H.264, HEVC, AV1, and VP9. |
| AMF | Windows, on AMD | H.264, HEVC, and AV1. |
| QSV | Intel. Frames come from memory only. | H.264, HEVC, AV1, and VP9 at a bitrate. |
| VideoToolbox | macOS | H.264 and HEVC. ProRes is for proxies. |
| Vulkan | Any listed GPU | ProRes (`prores_ks_vulkan`), for proxies only. |

`encode::detect` test-encodes with each encoder, each pixel format, and each GPU.
`export::detected` keeps the result in `encoders.json`.
The key is the FFmpeg versions, the system, and the GPUs and drivers.
`encoders.list` with `refresh` tests again.
`encoders.testing` names the process that is testing.
A name left by a process that died makes the next test skip the Vulkan encoders.
A job takes the encoders of the GPU in use (`Availability::in_use`).

## Deinterlacing and HDR

The reader deinterlaces as the CPU filter `bwdif` does.
CUDA frames use `bwdif_cuda`.
Vulkan frames use `bwdif_vulkan`.
`hw::deinterlace` checks, once for each process, that the filter gives the `bwdif` frames of that layout.
Other interlaced frames come down for `bwdif`.
They reach an encoder from memory.

libplacebo (`CudaPictures`) converts HDR to SDR, or into PQ or HLG.
The shader uses the CPU math (`color::gpu_color`).
That path is for NVENC and for the Vulkan ProRes encoder.
The other GPUs fit a picture only when its colors stay (`color::kept`).
A conversion on those GPUs comes down to `Normalizer`.
The viewer shader converts each picture and tone-maps it with the export math (`color::shader_colors`).

## Why

The rule is the GPU first, wherever the GPU works correctly (AGENTS.md rule 9).
vidcut does not move work to the CPU because the CPU is faster.
The only exceptions are GPUs that give wrong results.
`GpuDecoder::why_not` names them.
Direct3D 11 on AMD goes wrong after a seek into H.264 with open GOPs.
VideoToolbox goes wrong after that same seek.
Vulkan is trusted on NVIDIA GPUs only.
A GPU deinterlacer is checked against test pictures first.
Some scalers are checked the same way.
Those scalers are the Vulkan scaler and the Direct3D 11 video processor.
A GPU step that fails leaves its file to the CPU.

## Code

- `crates/vidcut-av/src/gpus.rs`
- `crates/vidcut-av/src/hw/mod.rs`
- `crates/vidcut-av/src/hw/vaapi.rs`
- `crates/vidcut-av/src/hw/d3d11.rs`
- `crates/vidcut-av/src/hw/vt.rs`
- `crates/vidcut-av/src/hw/vulkan.rs`
- `crates/vidcut-av/src/hw/vulkan_scale.rs`
- `crates/vidcut-av/src/hw/deinterlace.rs`
- `crates/vidcut-av/src/cuda.rs`
- `crates/vidcut-av/src/cuda/keep.rs`
- `crates/vidcut-av/src/cuda/mix.rs`
- `crates/vidcut-av/src/cuda/share.rs`
- `crates/vidcut-av/src/route.rs`
- `crates/vidcut-av/src/decode.rs`
- `crates/vidcut-av/src/color.rs`
- `crates/vidcut-av/src/export/make.rs`
- `crates/vidcut-av/src/export/encode.rs`
- `crates/vidcut/src/preferences.rs`
- `crates/vidcut/src/export.rs`
- `crates/vidcut/src/gui/picture.rs`
- `crates/vidcut/src/gui/shared.rs`
- `crates/vidcut/src/gui/vulkan.rs`
