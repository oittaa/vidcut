# GPU

- vidcut decodes, scales, mixes, shows and encodes on the GPU where the GPU gives the same results as the CPU.
- One GPU does all of this work.
- A frame stays in GPU memory as far as the next step can take it. Then it comes down to memory.
- [playback](playback.md) and [export](export.md) tell about the pipelines around this. [gotchas](../gotchas.md) tells about the drivers.

```mermaid
flowchart LR
  dec["hw::GpuDecoder"] --> frame["frame on a route::Place"]
  frame -->|"Route::Kept"| kept["kept as decoded"]
  frame -->|"Route::Whole"| mem["memory"]
  kept --> screen["screen::Screen<br>picture.wgsl"]
  mem --> screen
  screen --> shown["viewer"]
  screen -->|"fs_encode"| planes["the encoder's planes"]
  planes -->|"CUDA, IOSurface"| genc["GPU encoder"]
  planes -->|"read back"| cpu["CPU encoders"]
  cpu -.->|"uploaded"| genc
```

## The GPU and the decoder

- `gpus::list` lists the GPUs once, by PCI ids and slot (`Gpu::id`). A Mac lists no GPUs.
- `gpus::in_use` is the GPU that `gpu.device` names. Else it is `gpus::automatic`: CUDA device 0, else the first GPU that is not NVIDIA's.
- `Gpu::memory_mb` comes from DXGI on Windows, and from the largest device-local Vulkan heap on Linux. Integrated GPUs on Linux have no value.
- `GpuDecoder::best` opens the decoder of the GPU in use:
  - NVDEC on NVIDIA, with Vulkan for ProRes.
  - Direct3D 11 on other GPUs on Windows. VA-API on other GPUs on Linux.
  - VideoToolbox on a Mac.
- `VIDCUT_HWDEC` sets the decoder for tests.
- The player uses that decoder. Exports, proxies, renders and tiles use `GpuDecoder::own`, a stream or a device of their own on the same GPU.
  - Why: on the player's decoder, their work waits behind playback.
- `gpu.acceleration` lets the GPU decode, scale and encode. `gpu.decoding` lets it decode. Both are on by default.
- `GpuDecoder::why_not` names the streams that the CPU decodes. Stills and image sequences decode on the CPU.
- A stream that the GPU's decoder refuses decodes on the CPU (`VideoReader::open_gpu_or_cpu`).

## Routes

- `route::Place` is where a picture is: `Memory`, or one of the GPU places of `hw::Kind`.
- `route::Facts` holds what the planner knows of a frame:
  - Its place.
  - The draw takes the frames of that place (`Gpu::gpu_pictures`).
  - The draw reads its layout (`normalize::displayable`).
  - Keeping the frames failed before.
- `route::plan` gives the `Route`. `Route::Kept` wins.

| `Route` | The picture is |
|---|---|
| `Route::Kept` | kept as decoded, for the draw |
| `Route::Whole` | copied down whole |
| `Route::Cpu` | already in memory |

- `export::make::Maker` follows the plan. The viewer, exports, proxies, thumbnails and the AI inputs use it.
- A keep that fails becomes a fact. After that, the frames of that layer come down.

## Each decoder's frames

In the viewer, exports, proxies and thumbnails:

| Decoder | Kept | Read by the draw |
|---|---|---|
| NVDEC (CUDA) | `cuda::keep`: a copy | through a buffer shared with wgpu (`SharedBuffer`) |
| Vulkan | `vulkan::keep`: as decoded | images imported into wgpu |
| VA-API | `vaapi::Keeper`: a copy (`vaCopy`) | DMA-BUFs imported into wgpu |
| Direct3D 11 | `d3d11::Keeper`: a copy in shareable textures | a few shared textures, each imported once (`d3d11::Shared`) |
| VideoToolbox | as decoded | IOSurface planes as Metal textures; AYUV64 as one RGBA texture |

- The viewer imports VA-API pictures only from its own GPU (`player::set_viewer_gpu`). From another GPU, the pictures come down.
  - Why: the viewer cannot import the pictures of another GPU.
- The draw takes NVDEC pictures only when its wgpu device is on CUDA's GPU, as it does for Vulkan pictures. On another GPU, the pictures come down.
  - Why: the buffer that Vulkan shares with CUDA is on one GPU.
- vidcut keeps facts of each VA-API device. `NO_COPY` and `WAYS` are kept by the address of the display. `SIZES` is kept by the device, the layout and the size.

- ProRes 4444 on VideoToolbox takes P416, which has no alpha plane ([todo](../todo/clip-alpha.md)).
- A VideoToolbox frame that comes down is mapped, not copied (`hw::to_read`).
- A layer keeps the picture that its planes hold: from memory, from NVDEC, or brought down. A redraw of the same picture copies nothing.
- A layer that brings its picture down brings down only its own frame (`alone_copied_down`).
- `hw::Turner` turns VideoToolbox frames for the scalers of macOS (`vt::Turn`, `transpose_vt`).
- Lanczos on Metal scales the input of the scaler and the frame interpolation of macOS to the size of their session (`lanczos::sized`). It also scales the output of the scaler to the size that it shows at.
  - Why Lanczos: the scaler of VideoToolbox keeps less detail.

The draw:

- The viewer, exports, proxies and thumbnails draw each frame with one shader (`screen::Screen`, `picture.wgsl`).
  - The AI upscalers and the frame interpolation take their RGB input from the same draw (`export::video::Single`).
  - That input stays on the GPU (`Sink::Held`).
- The draw writes an RGB plane as RGBA. `held::Pipes` packs it to three bytes a pixel (`rgb.wgsl`). An RGB frame in memory is packed the same way.
  - Why: a plane of one byte a texel is three times as wide as the picture. A wide picture then goes past the texture size limit of the GPU.
- It mixes the dissolves, the wipes, the composite modes, the placed pictures, the titles and the subtitles.
- `export::video::Drawer` draws for the export, proxies, thumbnails and the AI inputs.
  - All of them share one wgpu device (`screen::offscreen`). It is on the GPU in use (`gpus::in_use`).
    - Why: the frames of the decoder stay on the GPU only when the draw is on the same GPU.
    - vidcut makes the device at the first draw on that GPU.
    - When wgpu has no adapter on that GPU, the device opens on the adapter that wgpu picks. vidcut prints which.
    - After a change of `gpu.device`, the AI upscalers and the frame interpolation start again on the new device (`start_on_offscreen`).
  - The viewer's frame render, the AI upscalers and the frame interpolation use that device too (`compute::Gpu`).
  - It opens with all the limits of the GPU. Why: the AI kernels need large buffers and much workgroup memory.
  - It is not the device of the viewer: a submit from another thread can make the viewer's surface fail ([gotchas](../gotchas.md#wgpu-and-egui)).
- There, `fs_encode` writes the planes of the encoder ([export](export.md#timeline-colors)).
  - NVENC takes them through memory that Vulkan shares with CUDA (`Shared::out`).
  - VideoToolbox takes them in the IOSurfaces of its own frames (`Sink::Surface`).
  - The other encoders take them from memory (`Sink::Memory`).

Frames that come down:

- CPU encoders take frames in memory.
- The player brings a frame down when the viewer cannot show its kind, when an import fails, or when its pool leaves the budget.
  - After an import or a copy into the shared buffer fails, that layer's frames come down. When a new decoder opens for the layer, the draw tries again (`Decoding`).
  - Why: a failure can be for a short time only, for example when an AI model holds the video memory.
- A picture in memory goes up into the frames of a GPU encoder (`Upload`, `CudaFrames::upload`).
- The player brings the pictures of the AI upscalers and the frame interpolation down (`held::in_memory`). Why: the viewer draws on another device.
  - They go to the viewer as RGB24, with no conversion on the CPU (`normalize::displayable`). An RGB24 still goes to the draw as it is too.
- The cut test of a retime brings down a 96×54 luma picture of each frame, not the frame (`held::small_luma`, [export](export.md)).

## Encoders

| Encoder | GPUs | Codecs |
|---|---|---|
| NVENC | NVIDIA | H.264, HEVC, AV1 |
| VAAPI | Linux, not NVIDIA | H.264, HEVC, AV1, VP9 |
| AMF | Windows, AMD | H.264, HEVC, AV1 |
| QSV | Intel, from memory | H.264, HEVC, AV1; VP9 at a bitrate |
| VideoToolbox | macOS | H.264, HEVC, ProRes |
| Vulkan | any listed GPU | ProRes, for proxies only |

- `encode::detect` test-encodes each encoder, layout and GPU that `encode::probes` lists.
- `export::detected` keeps the results in `encoders.json`, for the FFmpeg versions, the system and the GPUs.
- It tests again when the file lacks a test of `encode::probes`, and on `encoders.list` with `refresh`.
- `encoders.testing` names the process that tests. When that process dies, the next test skips Vulkan.
- Jobs use the encoders of the GPU in use (`Availability::in_use`).

## Super Scale

Super Scale (`SuperScale`) sets the upscaler of a picture that is enlarged.

- `render::super_scale_of` gives each piece its upscaler: its own, or the project's (`MediaUpscaler`, `ProjectUpscaler`). Auto is the scaler of macOS on macOS, and AI Best elsewhere.
- Classic enlarges in the shader of the viewer and the export, with the kernel of the picture's `Sharpness` (`screen::kernel`).
- `layer_size` lets the picture of an AI upscaler grow past its source size.
- The result of an AI upscaler is its own. It is not meant to match the CPU.

The AI upscalers share one path in the reader:

- `Source::ai_picture` finds the size to make (`super_scale::made_size`).
- A picture has a key (`PictureId`). It comes from a run of the upscaler (`Session`, `super_scale::Run`), which starts at `super_scale::first_frame`.
- A run takes frames in order. It reads them from the reader's queue, or with a second `VideoReader` (`Source::seek_ahead`).
- The run gives each picture once, and keeps the last one for a change of size.
- A picture of AI Fast or AI Best stays on the GPU as RGB (`held::Held`). The export draws it there (`Layout::Rgb`).
- The reader drops a run for a frame that the run does not give (`RUN_JUMP`).
- `super_scale::report` tells what the upscaler makes. The stats panel shows it (`SuperScaleStats`).

GPU memory:

- Each `Source` that uses an AI upscaler holds a `super_scale::User`.
- When the last user goes, `super_scale::Slot::release` frees the GPU memory of that upscaler.
  - Why: two AI upscalers do not hold memory at the same time.
- An AI upscaler borrows the GPU memory of the viewer's kept pictures (`player::lend_on`, `player::room_on`, `store::Lent`).
  - Why: if they do not fit together, the driver moves GPU memory to main memory, and a picture can take minutes.
- AI Best also borrows main memory for its caches (`player::lend_memory`, `store::LentMemory`).

AI Fast (`nanovsr`):

- NanoVSR 5.4M in WGSL shaders on the shared off-screen device.
- It works in chunks on a fixed grid of the file's frames (`nanovsr::FRAMES`). Thus the viewer and an export get the same pictures.
- It gives the asked frame first. A thread (`vidcut-upscale-rest`) makes the rest of the chunk.
  - Why: the viewer shows the frame after about half the work.
- Its 3×3 convolutions use Winograd (`wino_shader`). Why: they do most of the work.
- While the GPU makes a frame, the CPU reads the frame before it. Why: other GPU work fills the waits.

AI Best (`flashvsr`):

- FlashVSR+ Turbo on the shared off-screen device. A run is a `flashvsr::Stream` with no last frame.
- `flashvsr::check` refuses media that it cannot take, and sizes that do not fit (`run::plan`).
- It looks at the stop flag (`cancel::stop`) before each block. Why: a picture that the viewer no longer needs stops soon.
- Its KV caches stay on the GPU while they fit in the room (`run::plan`). Else some stay in main memory between chunks (`Keys::Host`).
  - `Stream::fit` measures the room before each chunk. Why: the encoder takes GPU memory after the plan.
- One matrix multiply shader runs its linear layers and most convolutions (`gemm::run`).

The scaler of macOS (`macos`):

- It is a `VTFrameProcessor` session with a `VTSuperResolutionScalerConfiguration`, on macOS 26 and later.
- `macos::check` refuses HDR media and pictures over `macos::MOST`.
- `macos::needed` and `macos::get_ready` let macOS download its models.
- A VideoToolbox frame reaches it on the GPU (`Run::push_frame`). A turned frame turns first (`hw::Turner`).
- It enlarges only by the factors of macOS. Then Lanczos makes the picture at the size that it shows at (`super_scale::made_size`).
  - Why: a picture of 4 times the source fills the cache of the viewer.
  - A picture of a layout that Lanczos cannot take comes out as P416 (`lanczos::takes`).
- Its buffers lose the color labels of the transfers. Why: with labels, it makes less detail.
- AI Fast and AI Best are off on macOS (`super_scale::off_here`).

## Frame interpolation

`interpolate` makes the frames of Retime Process ([export](export.md#which-source-frame), [deviation X93](../deviations.md#x93-retime-process-has-ai-models-in-the-place-of-optical-flow)).

- `interpolate::between` takes two RGB pictures on the GPU and a weight. It gives an RGB picture on the GPU.
- It runs on the shared off-screen device (`compute::Gpu`), with kernels of its own (`interpolate::ops`).
  - Its kernels and memory go when the last `interpolate::User` goes.
- The last pair stays on the GPU with its features. Why: the frames between one pair share them.
- AI Fast is RIFE 4.25 (`rife`). AI Best is EMA-VFI (`ema`). Frame Blend mixes on the same device (`Ops::combine`).
- EMA-VFI plans its work buffers in a pool (`ops::arena`), and refines in bands of rows (`ema::Pair`).
- The weights come from the authors' PyTorch files (`weights::Torch`).

On macOS (`interpolate::macos`):

- The frame rate conversion of macOS makes the frame, in a `VTFrameProcessor` session.
- Two VideoToolbox frames of a clip with no placed picture reach it on the GPU (`interpolate::takes_frames`). Turned frames turn first (`hw::Turner`).
- Lanczos sizes them to the size that the picture fills in the frame, before the frame is made.
- The made frame stays a VideoToolbox frame (`interpolate::between_frames`).
- Frames that the CPU decoded, and the frames of a placed clip, go through RGB.

`interpolate::report` tells what each process makes. The stats panel shows it (`RetimeStats`).

## Deinterlacing and HDR

- CUDA frames use `bwdif_cuda`, and Vulkan frames use `bwdif_vulkan`.
  - `hw::deinterlace` first checks that the filter gives the frames of `bwdif`. Else the frames come down for `bwdif`.
- `route::Facts::deinterlacing` gives the facts of a frame for the deinterlacer, and `route::plan` gives its route:
  - `Route::Kept`: the GPU filter deinterlaces the frame.
  - `Route::Whole`: the frame comes down for `bwdif`.
  - `Route::Cpu`: `bwdif` deinterlaces the frame in memory.
  - A GPU filter that does not build becomes a fact. After that, the frames come down.
- The shader of the draw converts each picture into the timeline color space (`color::shader_colors`).
- `fs_encode` converts the timeline color space into the color space of the output.

## Why

- The GPU goes first wherever it gives correct results (AGENTS.md rule 9).
- `GpuDecoder::why_not` names the GPUs that give wrong results:
  - AMD's Direct3D 11 and VideoToolbox go wrong after a seek into H.264 with open GOPs.
  - vidcut trusts Vulkan on NVIDIA only.
- vidcut checks GPU deinterlacers against test pictures first.
- A GPU step that fails leaves its file to the CPU.

## Code

- `crates/vidcut-av/src/gpus.rs`
- `crates/vidcut-av/src/hw/`
- `crates/vidcut-av/src/cuda/`
- `crates/vidcut-av/src/lanczos.rs`
- `crates/vidcut-av/src/screen/`
- `crates/vidcut-av/src/metal.rs`
- `crates/vidcut-av/src/super_scale/`
- `crates/vidcut-av/src/interpolate/`
- `crates/vidcut-av/src/route.rs`
- `crates/vidcut-av/src/decode.rs`
- `crates/vidcut-av/src/color.rs`
- `crates/vidcut-av/src/export/make.rs`
- `crates/vidcut-av/src/export/encode.rs`
