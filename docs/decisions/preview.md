# The preview: vidcut's own player, drawn with wgpu

The GUI previews through the player engine ([player.md](player.md)) and draws with eframe on wgpu.

Tests: `crates/vidcut/tests/live.rs` (the live session; no window, GPU or audio device), `gui.rs` (`--features vidcut/gui-tests`, needs a display), `picture.rs` (the shader against the export's conversion, on any wgpu adapter). The engine's tests: player.md.

## 1. What plays

- **Plans.** The timeline plays `render::flatten_lanes(project, 0, end)`; the source plays `render::tape` (one clip, or the Source Tape). These are the plans export renders, and the engine reads them with the export's readers. So every picture is the one `frame render` makes, for MPEG-TS, stills, image sequences, interlaced, rotated and HDR sources alike.
  - Where the project plays proxies, fresh proxies replace their media's pictures (§6).
  - One frame of black and silence follows the content, for the playhead to stand on at the end (after an append, decision D15).
- **One player for both** (`live/player.rs`). One engine plays whichever plan the viewer shows. The session keeps the other plan and its playhead, and the engine keeps that frame's picture ready (`Player::prepare`, player.md §3).
  - Switching (Q, `viewer.set`) pauses, plays the other plan from its playhead (`set_plan_at`) and shows its picture at once. Then the frame just left is prepared.
  - An edit function in the source viewer moves the timeline's playhead (decision D15). The new timeline plan and frame are prepared the same way, so Q shows the result at once.
  - `player.state`'s `ready` says whether each one's picture is decoded. The GUI's snapshot changes when the prepared picture lands.
- **Memory:** `Budget::automatic`, shared by the source and the timeline (player.md §3). The preferences' `player.memory_mb` and `player.video_memory_mb` (File > Preferences: Player; `0`, the default, means automatic) set other limits. A running GUI applies them at once.
- **Edits** give the engine a new plan (`set_plan`, player.md §3). Reloads after edits coalesce (gui.md §7). A different sample rate or channel count makes a new engine, since its device was opened for them.

## 2. Threads

| Thread | Does | Never |
|---|---|---|
| **Session** (`live`) | owns the engine: loads plans, switches between the source and the timeline, seeks, steps, plays; asks for presents; waits for frames | decodes, draws |
| **Engine** (player.md §1) | decodes pictures ahead of the clock and the other plan's frame; mixes the sound | touches the GPU's queue |
| **Viewer** (`vidcut-viewer`, `gui/video.rs`) | draws the picture at the engine's clock; answers presents and viewer screenshots | waits for the UI |
| **UI** (eframe) | registers the viewer's target as an egui texture and shows it; window screenshots | draws a viewer picture |

**Why the viewer has its own thread:** on GNOME Wayland with NVIDIA's driver, presenting a hidden (minimized) window blocks the UI thread until the window shows again, with Mailbox and FIFO alike. Submissions to the same device from another thread go on meanwhile, so presents, seeks and screenshots don't depend on the UI. Wayland can't show a minimized window again on request, so `gui.quit` ends the process without the window if it hasn't closed within 5 s.

## 3. Decoding: on the GPU when there is one

- **What the GPU decodes.** While `gpu.acceleration` and `gpu.decoding` are on (the defaults), the GPU decodes every stream it can: H.264, HEVC, AV1 and VP9 where it supports them (`VideoReader::open_gpu_or_cpu`). Turning either preference off decodes everything on the CPU.
- **Which decoder:** the best the computer has (`vidcut_av::hw::GpuDecoder::best`):
  - NVDEC on an NVIDIA GPU;
  - else VA-API on Linux (Mesa's driver for AMD, Intel's own), on the first render node that isn't NVIDIA's;
  - else Direct3D 11 video decoding on Windows, on the first adapter that isn't NVIDIA's.

  FFmpeg's own decoders run on these through its hardware contexts, so every one gives the same frames.
- **On the CPU:** streams the GPU doesn't decode (ProRes, VP8, a profile the GPU lacks), stills, image sequences, and H.264 where AMD's Direct3D 11 decoder would take it (`GpuDecoder::decodes`, gotchas.md).
- **Why the GPU is the default:** a CPU with many cores decodes H.264 faster than NVDEC, but the GPU leaves the CPU free for other work, HEVC, AV1 and VP9 are costly in software, and on a CPU with few cores the GPU is faster for H.264 too. Long GOPs (keyframes seconds apart, as cameras, phones and the web make them) are slow to seek in on either, since a frame late in a GOP decodes after every frame from its keyframe. That is what proxies are for (§6).
- **Kept in video memory.** A frame the GPU decoded stays there (NV12 or P010), and so do the pictures the engine keeps, within the video memory budget (player.md §3). Copying 4K frames down to memory is costly, so the viewer shows them from video memory where it can. Kept frames must not starve the decoder of surfaces:
  - FFmpeg copies each NVDEC frame out of the decoder's few surfaces into a frame of its own.
  - VA-API's decoder pool grows as needed (FFmpeg fixes its size only before libva 1.0), so a kept VA-API frame is the decoder's surface itself.
  - Direct3D 11's frames are copied out of the decoder's texture array into shareable textures of their own (`hw::Keeper`), and the copy is waited for, because the device that reads them next can't wait for Direct3D 11's work. A frame whose copy fails is copied down instead.
- **Shown from video memory**, with no copy through the CPU:
  - **NVDEC** (`gui/shared.rs`, `vidcut_av::cuda::share`): wgpu's Vulkan device makes a buffer and exports its memory (`VK_KHR_external_memory_fd`; on Windows `VK_KHR_external_memory_win32`), and CUDA imports it. To show a picture, CUDA copies its planes into the buffer and wgpu copies them on into the plane textures (§4). A few such buffers take turns; each is free again once the queue has done the copy out of it (`Queue::on_submitted_work_done`). Waiting with `Device::poll` instead would hold wgpu's device and stall the UI thread's frames.
  - **VA-API** (Linux, `gui/picture.rs`): the surface is exported as DMA-BUFs (FFmpeg's DRM PRIME map, which waits for the GPU's work on it). Each plane is imported as a texture of wgpu's Vulkan device with its DRM format modifier (`texture_from_dmabuf_fd`; Intel's are Y-tiled). An acquire from the external queue family takes the textures over from VA-API (`acquire`): wgpu can't record that barrier, and its own first barrier, from `UNDEFINED`, lets a driver drop a compressed picture. The planes are then copied into the plane textures. The picture is held until that copy is done, so its surface isn't reused under it.
  - **Direct3D 11** (Windows): wgpu's Vulkan device on the same adapter (matched by LUID) opens the texture through an NT handle (`texture_from_d3d11_shared_handle`, as NV12, or P010 where the device has P010 textures) and takes it over with the same acquire as VA-API. The shader draws from views of its two planes, so nothing is copied.
  - A picture with odd sides is in a texture rounded up to even sides; the shader samples only the picture's part.
- **Copied down** where the viewer can't show them from video memory: NVDEC's with a wgpu backend other than Vulkan, CUDA on a different GPU from the display's, a player without a viewer (tests); VA-API's and Direct3D 11's where the display can't import DMA-BUFs or Direct3D 11 textures, or an import fails (another GPU's). The frame comes to memory at its own size with its properties and is uploaded (§4). Intel's surfaces are copied down by mapping them, where that is quicker (`hw::download`, gotchas.md). Interlaced frames from VA-API and Direct3D 11 come down before `bwdif`; NVDEC's go through `bwdif_cuda`.
- **Switching.** The session watches the preferences file. A change, from anywhere, switches the engine's decoder at once (`Player::set_hardware`), which decodes its pictures again, the one shown first. `player.state`'s `hwdec` says what decoded the picture shown: `nvdec`, `vaapi`, `d3d11va` or `no`.
- **Frame-exact:** every GPU decoder's pictures are the CPU's frames, within two levels per pixel (`crates/vidcut-av/tests/player_gpu.rs` checks every decoder the machine has, holding to the GPU every codec whose profile the driver lists: `GpuDecoder::hardware_decodes`).

## 4. The picture on screen

- **One pass** (`gui/picture.rs`, `picture.wgsl`). A picture's planes go into textures at its own size, uploaded from memory or copied on the GPU from video memory (§3): Y, then U and V (8-bit planar) or interleaved UV (NV12; P010 in 16-bit textures where the adapter has `TEXTURE_FORMAT_16BIT_NORM`, otherwise as its high bytes). One draw into an RGBA8 target the size of the viewer's picture in physical pixels:
  - **placed** as export fits it (`Picture::placement`), black around it;
  - **rotated** upright by the display matrix (`Picture::rotation`);
  - **scaled in YUV**, as export scales before converting: going down, the average of up to 8×8 bilinear taps over the samples a pixel covers; going up, bilinear;
  - **converted** with the export's math (`vidcut_av::color::shader_colors`): the picture's matrix and range to BT.709 RGB; HDR (PQ, HLG) to linear light with reference white at 1.0, BT.2020 to BT.709, the mobius curve on the brightest channel, and BT.709's transfer as a 2.4 power. This is the GPU export shader's math (export.md §5), in 32-bit floats throughout.
- **Shown one to one.** egui shows the target as a native texture, nearest-sampled, in a rectangle rounded to physical pixels. The target is redrawn only when the engine's picture or the size changes.
- **Against the export** (`tests/picture.rs`): at a picture's own size, flat areas (PQ and HLG included) are within one level of `Picture::to_rgba`, the export's filters. Edges differ: at its own size the export repeats each chroma sample where the shader interpolates; scaled, swscale's bicubic is sharper than the shader's box. Barcodes read the same at every size.
- **Present mode:** Mailbox (or Immediate where there is one), so a new picture shows at the next refresh and no frames queue.

## 5. When a frame is on screen

A seek, step, load or edit reload returns once its frame is on screen. The contract is [gui.md §4](gui.md). Held keys: player.md §2.

## 6. Proxies

Rules, where the files live, and what still reads the originals: spec D41 and deviation X18. Jobs: gui.md §9. The GPU encode path, including `open_cuda` and `scale_cuda`: export.md §5.

- Each proxy frame keeps the source frame's presentation timestamp (in the source time base, or in microseconds when that base is not a sane clock).
- The viewer's plan swaps only the video path (`vidcut_core::preview::apply_proxies`).
- An image sequence's proxy has picture `k` at `k/rate`, with origin 0.
- **Decoded on the GPU, encoded on the CPU** (any encoder but NVENC). The proxy decodes on the decoder the viewer uses (`GpuDecoder::best`: NVDEC, VA-API, Direct3D 11), on a stream or device of its own. Frames are shrunk on the GPU to the proxy's size, in their own orientation and layout, and come down that small for the CPU to rotate, convert and encode (`hw::Shrinker`).
  - NVDEC's scale with `scale_cuda` in halving steps (export.md §5); VA-API's with `scale_vaapi`.
  - Direct3D 11's scale on the GPU's video processor (`hw::d3d11::Scaler`), because FFmpeg's `scale_d3d11` fails on AMD (gotchas.md). It is told the frames' matrix in and out as full range where DXGI has such a color space, so it keeps every level (except PQ, gotchas.md). It is used only once it has returned a test picture of grays and colors unchanged.
  - Where a GPU won't scale a layout, the frames come down whole.
- Tests: `crates/vidcut-av/tests/proxy.rs` (`--features vidcut-av/gpu-tests` for the GPU paths); `tests/live.rs` plays a proxy in the viewer.

## 7. Sound

Mix, clock, backward play and other speeds: player.md §4. Scrubbing: spec D42.

- **The device won't open:** the engine plays to a silent stand-in, and the status bar says why, once. The picture plays all the same.
- **Faders while dragged** (`mixer.preview`): a gain on top of the lane while the timeline plays (player.md §4). While the source is shown, the gain is held, and heard again when the timeline is shown. Releasing the fader saves the level with `track.set`. A track with no sound in the mix (muted, left out by solo, empty) is an error; when the source is shown the answer is `live: false`.
- **Monitoring mute** (`player.mute`) silences playback only (spec §9).

## 8. Exact frames for the two-up: decoded once either way

The Trim Editor's and Dynamic Trim's two-up show frames of a copy of the project that the player doesn't play. `vidcut_av::export::renderer::StillRenderer`, the export engine's renderer, draws them at the cell's size (`gui/stills.rs`), on a thread per cell. The engine's windows going back follow the same rule (player.md §3).

- **Why:** stepping back with exact seeks decodes each step from the keyframe before it. With keyframes seconds apart, that is a few hundred frames for every step.
- **Forward:** the file and decoder stay open, and the next frame decodes on from the one before.
- **Back:** a window renders at once, from a keyframe up to the frame asked for, and is kept.
  - The keyframe is found with one seek and one decoded frame (where the seek lands). The window starts at the first frame sampled at or after it, so a seek for it lands on that keyframe and nothing in the window decodes twice.
  - A window reaches back over more keyframes to hold at least 2 s, and holds at most half the memory budget (256 MB a cell).
- **Ahead:** windows render on a second thread (`Prefetch`) before they are needed. Wherever the position is, even after a jump, at least 2 s behind it are kept. Going back, one whole window more is kept, so the next window back renders while the position moves through this one. Asked for a frame a window is rendering, the renderer waits for it instead of rendering it too.
- **Floor:** the cell says where the shown stretch of the file starts. Windows never reach behind it, since frames before a clip's first one aren't shown there.
- **On the GPU:** while `gpu.acceleration` and `gpu.decoding` are on, HEVC, AV1 and VP9 decode on the GPU the viewer decodes on (`StillRenderer::decoding_on`, on a stream or device of their own; the preferences are read for each frame asked for) and are shrunk there to the cell's size. Interlaced frames from VA-API and Direct3D 11 come down whole, for the CPU to deinterlace. H.264 stays on the CPU, as for the timeline's tiles (gui.md §11).
- **Eviction:** over the budget, the pictures kept or looked at least recently go first. Moving back and forth over a cut keeps both sides whole: a gap in the kept frames would decode from a keyframe while the cell waits.
- Tests: `crates/vidcut-av/tests/renderer.rs`, `renderer_gpu.rs` (`--features vidcut-av/gpu-tests`).
