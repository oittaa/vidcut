# The preview: vidcut's own player, drawn with wgpu

The GUI previews through the player engine ([player.md](player.md)) and draws with eframe on wgpu. Tests: `crates/vidcut/tests/live.rs` (the live session, headless), `gui.rs` (`--features vidcut/gui-tests`, needs a display), `picture.rs` (§4).

## 1. What plays

- **Plans.** The timeline plays `render::flatten_lanes(project, 0, end)`, the source `render::tape` (a clip, or the Source Tape): the plans export renders, read with its readers, so every picture is the one `frame render` makes, proxies included (§6). One frame of black and silence follows, for the playhead to stand on (decision D15).
- **One player for both** (`live/player.rs`). The session keeps the plan not shown and its playhead; the engine keeps that frame's picture prepared (player.md §3), so switching (Q, `viewer.set`) and edits from the source viewer show their frame at once. `player.state`'s `ready` says whether each is decoded.
- **Edits** give the engine a new plan (`set_plan`; reloads: gui.md §7). Another sample rate or channel count makes a new engine.

## 2. Threads

| Thread | Does | Never |
|---|---|---|
| **Session** (`live`) | owns the engine: plans, viewer switches, seeks, steps, play; asks for presents | decodes, draws |
| **Engine** (player.md §1) | decodes pictures, mixes the sound | touches the GPU's queue |
| **Viewer** (`vidcut-viewer`, `gui/video.rs`) | draws the picture at the engine's clock; answers presents and viewer screenshots | waits for the UI, which presenting a hidden window can block until it shows again (NVIDIA on Wayland) |
| **UI** (eframe) | shows the viewer's target as an egui texture; window screenshots | draws a viewer picture |

## 3. Decoding: on the GPU when there is one

- **What.** While `gpu.acceleration` and `gpu.decoding` are on (the defaults), the GPU decodes every stream it can: H.264, HEVC, AV1 and VP9 (`VideoReader::open_gpu_or_cpu`). The CPU decodes the rest (ProRes, VP8, profiles the GPU lacks), stills, image sequences, and H.264 where AMD's Direct3D 11 decoder would take it (gotchas.md).
- **Which decoder** (`hw::GpuDecoder::best`): NVDEC on NVIDIA, else VA-API on Linux, else Direct3D 11 on Windows, the last two on the first device that isn't NVIDIA's. FFmpeg's own decoders run on them, so every one gives the same frames.
- **Why the GPU by default:** it leaves the CPU free, HEVC, AV1 and VP9 are costly in software, and on a CPU with few cores it is faster for H.264 too. Long GOPs are slow to seek in either way; proxies are for that (§6).
- **Kept in video memory** (NV12 or P010) within its budget (player.md §3), since copying 4K frames down is costly. Kept frames must not starve the decoder: FFmpeg copies NVDEC's out of its few surfaces, VA-API's pool grows, and Direct3D 11's are copied into shareable textures (`hw::Keeper`).
- **Shown from video memory**, with no copy through the CPU: NVDEC's through memory that wgpu's Vulkan device exports and CUDA imports (`gui/shared.rs`); VA-API's surfaces as DMA-BUFs and Direct3D 11's textures by NT handle (same adapter), imported into wgpu's Vulkan device and taken over by an acquire from the external queue (`gui/acquire.rs`).
- **Copied down** where the viewer can't show them from there (another backend or GPU, a failed import, no viewer), then uploaded. Interlaced VA-API and Direct3D 11 frames come down for `bwdif`; NVDEC's use `bwdif_cuda`.
- **Switching.** The session watches the preferences file; a change switches the engine's decoder at once (`Player::set_hardware`), which decodes its pictures again. `player.state`'s `hwdec`: `nvdec`, `vaapi`, `d3d11va` or `no`.
- **Frame-exact:** every GPU decoder's pictures are the CPU's frames within two levels (`player_gpu.rs`, for every decoder and codec the machine lists).

## 4. The picture on screen

- **One pass** (`gui/picture.rs`, `picture.wgsl`). One draw into an RGBA8 target of the viewer's size in physical pixels places the picture as export fits it, rotates it upright, scales it in YUV as export does (a box down, bilinear up) and converts it with the export's math (`color::shader_colors`, export.md §4) in 32-bit floats.
- **Shown one to one:** egui shows the target as a native texture, nearest-sampled, on physical pixels; it is redrawn only when the picture or the size changes. Mailbox (or Immediate) presents, so no frames queue.
- **Against the export** (`tests/picture.rs`): flat areas within one level of `Picture::to_rgba`. Edges differ (the export repeats chroma samples; swscale's bicubic is sharper than the box), but barcodes read the same at every size.

## 5. When a frame is on screen

A seek, step, load or edit reload returns once its frame is on screen: gui.md §4.

## 6. Proxies

- **Rules**, where the files live and what still reads the originals: spec D41, deviation X18. Jobs: gui.md §9. Tests: `crates/vidcut-av/tests/proxy.rs` (`--features vidcut-av/gpu-tests` for the GPU), `tests/live.rs`.
- **Frames.** A proxy frame keeps its source frame's timestamp; an image sequence's proxy has picture `k` at `k/rate`. The viewer's plan swaps only the video (`vidcut_core::preview::apply_proxies`).
- **Decoded on the GPU, encoded on the CPU** (NVENC: export.md §5): on the viewer's decoder, shrunk there in its own orientation and layout (`hw::Shrinker`: `scale_cuda`, `scale_vaapi`, or Direct3D 11's video processor, since FFmpeg's `scale_d3d11` fails on AMD, gotchas.md). Layouts a GPU won't scale come down whole.

## 7. Sound

- **The mix, the clock and speeds:** player.md §4; scrubbing: spec D42. A device that won't open gets a silent stand-in, and the status bar says why, once.
- **Faders while dragged** (`mixer.preview`) put a gain on the lane while the timeline plays, held while the source is shown; releasing saves the level with `track.set`. A track with no sound in the mix (muted, left out by solo, empty) is an error.
- **Monitoring mute** (`player.mute`) silences playback only (spec §9).

## 8. Exact frames for the two-up: decoded once either way

The Trim Editor's and Dynamic Trim's two-up show frames of a project copy the player doesn't play: `export::renderer::StillRenderer` draws them at the cell's size, a thread per cell (`gui/stills.rs`). Exact seeks back would decode each step from its keyframe. Tests: `crates/vidcut-av/tests/renderer.rs`, `renderer_gpu.rs` (`--features vidcut-av/gpu-tests`).

- **Forward** the decoder stays open and decodes on. **Back**, a window from a keyframe up to the frame renders at once and is kept: at least 2 s, at most half the budget (256 MB a cell), never behind the cell's floor (the start of the stretch shown). The player's windows follow this rule (player.md §3).
- **Ahead,** a second thread (`Prefetch`) keeps at least 2 s behind the position, and one window more while going back.
- **On the GPU,** HEVC, AV1 and VP9 decode on the viewer's decoder (`StillRenderer::decoding_on`) and shrink there to the cell; H.264 stays on the CPU, as for the timeline's tiles (gui.md §11).
- **Eviction:** the least recently kept or looked at go first, so going back and forth over a cut keeps both sides whole.
