# GPU frames: one pipeline

**When:** before another GPU path is added: VideoToolbox frames kept on the GPU, QSV from GPU memory, Vulkan Video (own-player.md).

**The goal:** a frame stays in GPU memory wherever that is possible, from the decoder to the encoder or the display. That holds even where a fast CPU, such as the M4's, would be a little quicker. A desktop computer is fast enough either way. A laptop must be fast enough too, but it saves its battery when the GPU does the work. Past "fast enough", speed doesn't count: 200 fps or 1,000 doesn't matter. What counts is that frames aren't copied between the GPU and memory, and how much CPU and power a job takes.

Today that holds for NVIDIA alone, and for exports, not proxies, on VA-API and AMF. Each path was added for one GPU and one job.

## Today

Three mechanisms move frames, each with its own switch on the kind of GPU:

| Mechanism | Does | GPUs | Used by |
|---|---|---|---|
| `cuda::CudaPictures` | NVDEC's frames turned and converted (libplacebo on Vulkan), scaled and padded, for NVENC | NVIDIA | exports, proxies |
| `hw::Fitter` | frames turned, scaled and padded on the decoder's device, for an encoder there | VA-API, Direct3D 11 | exports only |
| `hw::Shrinker` | frames scaled on the GPU, then copied down (`export::video::down`) | VA-API, CUDA, Direct3D 11 | proxies, exports to encoders that take frames from memory, tiles, the two-up, thumbnails |

Around them:
- **The choice is made per job, by encoder name.** Exports take the CUDA path when the encoder is NVENC (`gpu_output` in export/mod.rs). They fit on the GPU only for the pairs VAAPI with VA-API and AMF with Direct3D 11 (`encode` in export/mod.rs). Proxies check for NVENC again (`proxy::encode_into`).
- **Proxies have an encode loop of their own** (`proxy::encode_into`), beside the export's (`export::video::VideoFrames`), so they don't get the export's GPU fitting.
- **Light measurements and analyses come down whole**, on purpose: their values must be the CPU's to the bit (export.md §4).
- **The player keeps frames on the GPU** (`hw::Keeper`), and the GUI imports them for each GPU (gui/shared.rs, gui/picture.rs). Where a display can't take them, they come down whole (`player::frame`).
- **VideoToolbox is in none of the three.** On a Mac every frame comes down whole and the CPU scales it: `hw::Shrinker` finds no scaler for VideoToolbox's frames.

## Design

**Where a frame is:** memory, or a GPU's: CUDA, VA-API, Direct3D 11, VideoToolbox, Vulkan.

**What a job asks of a frame** (`Ops`): deinterlace, turn, scale to a size, convert colors (matrix, range, HDR to SDR), pad, and the layout the next step takes. Light measurements and analyses ask for whole frames in memory, as now.

**A backend for each place**, one module each, all with the same functions:
- `fit(frame, ops)`: the ops done there, with `can(ops, frame)` saying which it can do.
- `down(frame)` and `up(frame)`: to and from memory.
- `keep(frame)`: held for the player without running the decoder out of surfaces.
- `encoder_frames(encoder)`: the frames an encoder on its device takes, if it takes this place's frames at all.

The CPU is a backend too: its `fit` is today's `Normalizer`.

Each GPU's own code stays: `scale_cuda` and libplacebo, the `*_vaapi` filters, Direct3D 11's video processor, `scale_vt` and `transpose_vt`. Only its place moves, into its backend. Nothing outside the backends asks what kind of GPU it is.

**A planner** decides the route once for each piece of a job, as a pure function of the frame's place and properties, the job's ops, the places the encoder takes frames from, and the capabilities (below). It prefers, in order:
1. Everything where the decoder put the frame: fit there, encode from there.
2. Fit there what can be fitted, shrinking first, then copy down the small picture, finish on the CPU, and upload only if the encoder needs it.
3. Everything on the CPU.

It never leaves the GPU because the CPU would be faster. A route that works on the GPU wins.

The route goes into `VIDCUT_DEBUG` and into the result (`made_with`: `stayed on the GPU (VideoToolbox)`, `came down at 960×540`, `came down whole`).

**Capabilities** are tested once for each GPU and FFmpeg, and kept with `encoders.json`:
- What each decoder decodes (`hw::plays`, `vidcut decoders list`).
- What each backend fits, on test pictures as `hw::d3d11::Scaler` tests its levels today.
- Which encoders take which places' frames, by test encodes from GPU frames.

What a test can't settle is written as a fact with its source. For example, FFmpeg's `scale_vt` keeps the input's layout (vf_scale_vt.c), so 10-bit stays 10-bit on the GPU.

**One engine for every job.** Exports, proxies (an export at a small size), the timeline's tiles, the two-up, `frame render`, storyboards, thumbnails, light measurements and analyses all go through `VideoFrames` and the planner. The player's route ends in `keep`. Its import into the viewer (gui/shared.rs, gui/picture.rs) stays per GPU, behind the same place names.

## Steps

Each step is measured against the step before it on the RTX 4080 SUPER, the Radeon iGPU, the N150 and the M4. The measurement takes the same clips, the same run and a release build. It gives the frames copied each way, from the result's `copies`, and the CPU time first, then the time, which only has to stay fast enough.
1. **Planner and backends over today's code:** wrap `CudaPictures`, `Fitter`, `Shrinker`, `Keeper` and the `Normalizer` as backends. The planner reproduces today's routes exactly, and puts the route in `made_with`. No other behavior changes, and every test and barcode check passes unchanged.
2. **Proxies through the export engine:** drop `encode_into`'s own loop. VA-API and AMF proxies then fit on the GPU as exports do.
3. **VideoToolbox backend:** `scale_vt` and `transpose_vt`, with VideoToolbox's encoders taking its frames. Not known yet, and to be tested on the M4: whether the H.264 encoder takes 10-bit frames.
4. **Vulkan as a backend** (libplacebo) for colors and HDR where a GPU's own fitting lacks them, as NVIDIA's path already uses it. FFmpeg makes its Vulkan device from VA-API, DRM and CUDA devices only, so VA-API's frames can go to it through FFmpeg. Direct3D 11's need an import of vidcut's own, as the viewer has (gui/picture.rs).

## Tests

- **The planner:** unit tests, each one input and the route it must give.
- **Backends:** each must give the CPU's pictures, barcode for barcode and within two levels, checked by hand on each machine with the barcode fixtures as GPU decoders are now.
- **No copies where none are planned:** a script runs the jobs on the GPU in use, compares the result's `copies` with the route, and prints the base case beside the change.
