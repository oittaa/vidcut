# GPU frames: one pipeline

**When:** before another GPU path is added: VideoToolbox frames kept on the GPU, QSV from GPU memory, Vulkan Video (own-player.md, gpu-decoding.md).

**The goal:** a frame stays in GPU memory wherever that is possible, from the decoder to the encoder or the display. That holds even where a fast CPU, such as the M4's, would be a little quicker. A desktop computer is fast enough either way. A laptop must be fast enough too, but it saves its battery when the GPU does the work. Past "fast enough", speed doesn't count: 200 fps or 1,000 doesn't matter. What counts is that frames aren't copied between the GPU and memory, and how much CPU and power a job takes.

Today that holds for NVIDIA alone, and for exports, not proxies, on VA-API and AMF. Each path was added for one GPU and one job.

## Today

Three mechanisms move frames, each with its own switch on the kind of GPU:

| Mechanism | Does | GPUs | Used by |
|---|---|---|---|
| `CudaPictures` (cuda.rs:315) | NVDEC's frames turned and converted (libplacebo on Vulkan), scaled and padded, for NVENC | NVIDIA | exports (export/video.rs:466), proxies (proxy.rs:666) |
| `Fitter` (hw.rs:1185) | frames turned, scaled and padded on the decoder's device, for an encoder there | VA-API, Direct3D 11 | exports only (export/video.rs:1399) |
| `Shrinker` (hw.rs:1055) | frames scaled on the GPU, then copied down | VA-API, CUDA, Direct3D 11 | proxies, exports to encoders that take frames from memory, tiles, the two-up (export/video.rs:156) |

Around them:
- **The choice is made per job, by encoder name.** Exports take the CUDA path when the encoder is NVENC (export/mod.rs:326). They fit on the GPU only for the pairs VAAPI with VA-API and AMF with Direct3D 11 (export/mod.rs:768). Proxies check NVENC again (proxy.rs:664).
- **Proxies have an encode loop of their own** (proxy.rs, `encode_into`), beside the export's (`VideoFrames`, export/video.rs), so they don't get the export's GPU fitting.
- **Whole frames come down** in the light measurements (light.rs:865), thumbnails (thumbs.rs:252), analyses (analysis.rs:238) and the player's pictures for a display that can't take them (player/frame.rs:133).
- **The player keeps frames on the GPU** (`Keeper`, hw.rs:918), and the GUI imports them for each GPU (gui/shared.rs, gui/picture.rs).
- **VideoToolbox is in none of the three**, so on a Mac every frame comes down whole and the CPU scales it (hw.rs:1086).

## Design

**Where a frame is:** memory, or a GPU's: CUDA, VA-API, Direct3D 11, VideoToolbox, Vulkan.

**What a job asks of a frame** (`Ops`): deinterlace, turn, scale to a size, convert colors (matrix, range, HDR to SDR), pad, and the layout the next step takes.

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

**One engine for every job.** Exports, proxies (an export at a small size), the timeline's tiles, the two-up, `frame render`, storyboards, thumbnails and light measurements all go through `VideoFrames` and the planner. The player's route ends in `keep`. Its import into the viewer (gui/shared.rs, gui/picture.rs) stays per GPU, behind the same place names.

## Steps

Each step is measured against the step before it on the RTX 4080 SUPER, the Radeon iGPU, the N150 and the M4. The measurement takes the same clips, the same run and a release build. It gives the frames copied each way and the CPU time first, then the time, which only has to stay fast enough.
1. **Count and report:** count the frames copied down and up, and their sizes, for each job, and put the route in `made_with`. Measure the base case on all four machines.
2. **Planner and backends over today's code:** wrap `CudaPictures`, `Fitter`, `Shrinker`, `Keeper` and the `Normalizer` as backends. The planner reproduces today's routes exactly. No behavior changes, and every test and barcode check passes unchanged.
3. **Proxies through the export engine:** drop `encode_into`'s own loop. VA-API and AMF proxies then fit on the GPU as exports do.
4. **VideoToolbox backend:** `scale_vt` and `transpose_vt`, with VideoToolbox's encoders taking its frames. Not known yet, and to be tested on the M4: whether the H.264 encoder takes 10-bit frames.
5. **Whole-frame copies:** light measurements, thumbnails and analyses copy down only what they need, at the size they need.
6. **Vulkan as a backend** (libplacebo) for ops a GPU's own fitting lacks: colors and HDR on VA-API and Direct3D 11, as NVIDIA's already uses it.

## Tests

- **The planner:** unit tests, each one input and the route it must give.
- **Backends:** each must give the CPU's pictures, barcode for barcode and within two levels, checked by hand on each machine with the barcode fixtures as GPU decoders are now.
- **No copies where none are planned:** a script runs the jobs on the GPU in use, compares the frames-copied counters with the route, and prints the base case beside the change (`scripts/proxy-gpu-check.sh` grows into it).
