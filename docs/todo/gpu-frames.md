# GPU frames: one pipeline

**When:** before another GPU path is added: QSV from GPU memory, Vulkan Video (own-player.md).

**The goal:** a frame stays in GPU memory wherever that is possible, from the decoder to the encoder or the display. That holds even where a fast CPU, such as the M4's, would be a little quicker. A desktop computer is fast enough either way. A laptop must be fast enough too, but it saves its battery when the GPU does the work. Past "fast enough", speed doesn't count: 200 fps or 1,000 doesn't matter. What counts is that frames aren't copied between the GPU and memory, and how much CPU and power a job takes.

Today that holds for NVIDIA, and for VA-API, AMF and VideoToolbox where the encoder is on the decoder's device. Each path was added for one GPU and one job.

## Today

**The planner** (`route.rs`) picks each frame's route. A frame is made on the GPU where it is and handed on there (`Gpu`), shrunk there and copied down (`Shrunk`), copied down whole (`Whole`), made on the CPU from memory (`Cpu`), or kept as decoded for the player (`Kept`). `route::plan` is a pure function of facts about the frame (`Facts`) and of where the job's pictures go (`Dest`: memory, an encoder that takes frames on one place, or a display). The facts are where the frame is, whether its GPU fits it for an encoder there, whether its GPU hands the picture to an encoder on another place (`reaches`: CUDA's to Vulkan's encoder), whether the output shows it smaller, whether a display takes its layout, and whether fitting or keeping failed before. The engine's `export::make::Maker` plans each picture it makes, and `VIDCUT_DEBUG` says each file's route. `VideoFrames` and proxies make their pictures with it. A proxy's `made_with` says how its frames went (`route::Went`).

**The backends** say what they fit (`can_fit`) from `route::Ops`: the turn, the frame's and the output's layouts, RGB or YUV, whether the colors stay, whether the picture is placed on black, and whether the encoder's own frames are there to fit into. Three mechanisms are their work:

| Mechanism | Does | GPUs | Fits (`can_fit`) | Used by |
|---|---|---|---|---|
| `cuda::CudaPictures` | NVDEC's frames turned and converted (libplacebo on Vulkan), scaled and padded, for NVENC; made into Vulkan's ProRes encoder's pictures by libplacebo | NVIDIA | YUV turned by quarter turns | exports, proxies |
| `hw::Fitter` | frames turned, scaled and padded on the decoder's device, for an encoder there (VideoToolbox's not padded) | VA-API, Direct3D 11, VideoToolbox | frames in the output's colors; Direct3D 11's into the encoder's frames and not 8-bit into 10; VideoToolbox's in the output's layout, not placed on black | exports, proxies |
| `hw::Shrinker` | frames scaled on the GPU, then copied down (`export::normalize::down`) | VA-API, CUDA, Direct3D 11, VideoToolbox | | proxies, exports to encoders that take frames from memory, tiles, the two-up, thumbnails |

The CPU's backend is `export::normalize` (`Normalizer`).

Around them:
- **Where the encoder takes frames is chosen per job, by encoder name** (`encode` in export/mod.rs, `Writer::new` in proxy.rs). NVENC takes CUDA frames when a GPU decodes (`gpu_output`), a proxy's where the GPU's scaler builds for its file. Vulkan's ProRes encoder takes libplacebo's pictures where its own frames are on the Vulkan device the scaler builds on (`Output::Vulkan`). VAAPI, AMF and VideoToolbox open on the decoder's device only for the pairs VAAPI with VA-API, AMF with Direct3D 11 and VideoToolbox with VideoToolbox (`encode::shares`). The job's `Output` and `Dest` follow from that (`Output::for_encoder`).
- **Proxies have a loop of their own** (`proxy::encode_into`): it decodes every frame at its own time and gives each its frame's timestamp. They make their pictures with the `Maker` and open their encoders with `encode::open_video`, so VA-API, AMF and VideoToolbox proxies fit on the GPU as exports do.
- **Light measurements and analyses come down whole**, on purpose: their values must be the CPU's to the bit (export.md §4).
- **HDR footage's 10-bit proxies keep its colors**, so nothing is converted between the decoder and the encoder: NVDEC's frames go to NVENC, Direct3D 11's to AMF and VideoToolbox's to its encoders on the GPU. Its 8-bit proxies are tone-mapped: by libplacebo on NVIDIA's GPU, elsewhere on the CPU.
- **The player keeps frames on the GPU** (`hw::Keeper`, the `Kept` route), and the GUI imports them for each GPU (gui/shared.rs, gui/picture.rs). Where a display can't take them, they come down whole (`player::frame`).
- **VideoToolbox's backend is `hw/vt.rs`.** Its frames stay on the GPU for its encoders where they fill the picture in the output's layout and colors. The rest shrink with `scale_vt` before they come down. VideoToolbox's H.264 encoder takes 8-bit frames only (FFmpeg 9.0.2 lists `videotoolbox_vld`, `nv12` and `yuv420p`), so 10-bit pieces for H.264 come down. Its HEVC and ProRes encoders take P010, so 10-bit HEVC exports and the HEVC and ProRes proxies of 10-bit sources stay on the GPU.

## Design

**Where a frame is:** memory, or a GPU's: CUDA, VA-API, Direct3D 11, VideoToolbox, Vulkan.

**What a job asks of a frame** (`Ops`): deinterlace, turn, scale to a size, convert colors (matrix, range, HDR to SDR), pad, and the layout the next step takes. Light measurements and analyses ask for whole frames in memory, as now.

**A backend for each place**, one module each, all with the same functions:
- `fit(frame, ops)`: the ops done there, with `can_fit(ops)` saying whether it can.
- `down(frame)` and `up(frame)`: to and from memory.
- `keep(frame)`: held for the player without running the decoder out of surfaces.
- `encoder_frames(encoder)`: the frames an encoder on its device takes, if it takes this place's frames at all.

The CPU is a backend too: its `fit` is today's `Normalizer`.

Each GPU's own code stays: `scale_cuda` and libplacebo, the `*_vaapi` filters, Direct3D 11's video processor, `scale_vt` and `transpose_vt`. Only its place moves, into its backend. Nothing outside the backends asks what kind of GPU it is.

**A planner** decides each frame's route, as a pure function of the frame's place and properties, the job's ops, the places the encoder takes frames from, and the capabilities (below). It prefers, in order:
1. Everything where the decoder put the frame: fit there, encode from there.
2. Fit there what can be fitted, shrinking first, then copy down the small picture, finish on the CPU, and upload only if the encoder needs it.
3. Everything on the CPU.

It never leaves the GPU because the CPU would be faster. A route that works on the GPU wins.

The route goes into `VIDCUT_DEBUG` and into the result (`made_with`: `frames stayed there`, `frames came down at 960×540`, `frames came down whole`).

**Capabilities** are tested once for each GPU and FFmpeg, and kept with `encoders.json`:
- What each decoder decodes (`hw::plays`, `vidcut decoders list`).
- What each backend fits, on test pictures as `hw::d3d11::Scaler` tests its levels today.
- Which encoders take which places' frames, by test encodes from GPU frames.

What a test can't settle is written as a fact with its source. For example, FFmpeg's `scale_vt` keeps the input's layout (vf_scale_vt.c), so 10-bit stays 10-bit on the GPU.

**One engine for every job.** Exports, proxies (an export at a small size), the timeline's tiles, the two-up, `frame render`, storyboards, thumbnails, light measurements and analyses all go through the engine's `Maker` and the planner. Exports and proxies do already, proxies in a loop of their own. The player's route ends in `keep`. Its import into the viewer (gui/shared.rs, gui/picture.rs) stays per GPU, behind the same place names.

**Open:**
- `Ops` with deinterlacing and the size to scale to.
- `fit`, `down`, `up` and `keep` as each backend's own functions. Today they are `CudaPictures`, `hw::Fitter`, `hw::Shrinker`, `hw::Keeper` and the encoders' uploads, each choosing by pixel format.
- `encoder_frames`.
- Capabilities tested and kept with `encoders.json`.
- The route in an export's result.
- One engine for every job.
- NVDEC's 4:2:2 (NV16, P210) and VideoToolbox's NV16 shown from GPU memory. They come down: no GPU here decodes them to test it.

## Steps

Each step is measured against the step before it on the RTX 4080 SUPER, the Radeon iGPU, the N150 and the M4. The measurement takes the same clips, the same run and a release build. It gives the frames copied each way, from the result's `copies`, and the CPU time first, then the time, which only has to stay fast enough.
1. **Vulkan as a backend** (libplacebo) for colors and HDR where a GPU's own fitting lacks them, as NVIDIA's path already uses it. FFmpeg makes its Vulkan device from VA-API, DRM and CUDA devices only, so VA-API's frames can go to it through FFmpeg. Direct3D 11's need an import of vidcut's own, as the viewer has (gui/picture.rs). Its first piece makes ProRes proxies and decodes ProRes on NVIDIA's Vulkan device: [vulkan-decode.md](vulkan-decode.md).

## Tests

- **The planner:** unit tests, each one input and the route it must give.
- **Backends:** each must give the CPU's pictures, barcode for barcode and within two levels, checked by hand on each machine with the barcode fixtures as GPU decoders are now.
- **No copies where none are planned:** a script runs the jobs on the GPU in use, compares the result's `copies` with the route, and prints the base case beside the change.
