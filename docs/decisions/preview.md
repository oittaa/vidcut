# The preview: vidcut's own player, drawn with wgpu

**Status:** accepted (October 2026). The GUI previews through the player engine ([player.md](player.md)) and draws with eframe on wgpu.

**Evidence** (all rerunnable):
- [`crates/vidcut/tests/live.rs`](../../crates/vidcut/tests/live.rs): the live session with its one engine, a stand-in renderer and a recording stand-in device, no window, GPU or audio device. Seeks frame-exact at every boundary (also with a late renderer), overtaken seeks, seeks while playing and while an edit reloads, pictures against beeps as heard, every frame shown in order, audio scrubbing, backward play, the source viewers, switching between the source and the timeline (each one's frame kept ready, after an edit from the source too), stills and sequences, interlaced, HDR and turned clips, proxies, several tracks and faders, an unreadable file.
- [`crates/vidcut/tests/gui.rs`](../../crates/vidcut/tests/gui.rs) (`--features vidcut/gui-tests`, needs a display): the real window. Screenshots of the shader's picture against `vidcut frame render` (H.264, a turned clip, 10-bit HEVC on NVDEC), edits, a minimized window, playing, the GPU preference.
- [`crates/vidcut/tests/picture.rs`](../../crates/vidcut/tests/picture.rs): the shader against the export's conversion on any wgpu adapter (lavapipe too); NVDEC's pictures shown from video memory as they show copied down, to the byte (NV12, P010, deinterlaced, odd sizes).
- [`crates/vidcut-av/tests/player_gpu.rs`](../../crates/vidcut-av/tests/player_gpu.rs) (`--features vidcut-av/gpu-tests`): pictures decoded on NVDEC against the CPU's; the engine's own tests in player.md.
- `cargo run -p vidcut-av --example decode_speed -- FILE`: raw decoding speed on the CPU and on NVDEC.

## 1. What plays

- **Plans.** The timeline plays `render::flatten_lanes(project, 0, end)`, the source `render::tape` (one clip, or the Source Tape): the plans export renders, so every picture is the one `frame render` makes (the engine reads them with the export's readers). Fresh proxies replace their media's pictures where the project plays proxies (§6). A frame of black and silence follows the content, where the playhead stands at the end (after an append, decision D15).
- **One player for both** (`live/player.rs`). One engine plays whichever the viewer shows. The other keeps its plan and playhead in the session, and the engine keeps the picture there ready (`Player::prepare`, player.md §3): decoded while the engine is paused with nothing else to decode, and held whatever the budget. Switching (Q, `viewer.set`) pauses, plays the other plan from its playhead (`set_plan_at`), and shows its picture at once; then the frame left is prepared. An edit function from the source moves the timeline's playhead (decision D15): its new plan and frame are prepared the same way, so Q shows the result at once. `player.state`'s `ready` says whether each one's picture is decoded, and the GUI's snapshot changes when the one kept ready lands.
- **Memory:** the more pictures are kept, the less the viewer waits, so it uses what the computer has (`Budget::automatic`, player.md §3): pictures in memory up to the larger of 1 GB and half the computer's memory, pictures NVDEC decoded up to half the GPU's memory. The preferences' `player.memory_mb` and `player.video_memory_mb` (File > Preferences: Player; `0`, the default, is automatic) set other limits, which a running GUI takes at once. The source and the timeline share the budgets: after one played a while, the other's pictures going back may be gone, and its first step back decodes a window again (player.md §7).
- **Edits** give the engine a new plan (`set_plan`): it keeps its position, open files and kept pictures, and plays on if it plays. Reloads after edits coalesce as before (gui.md §7). Another sample rate or channel count makes a new engine (its device was opened for them).
- **Nothing to convert.** Index-less containers (MPEG-TS), stills, image sequences at any rate, interlaced sources, turned pictures and HDR play as export reads them; the engine needs no remuxes, image lists, filler media or patches at cuts.

## 2. Threads

| Thread | Does | Never |
|---|---|---|
| **Session** (`live`) | owns the engine: loads plans, switches between the source and the timeline, seeks, steps, plays; asks for presents; waits for frames | decodes, draws |
| **Engine** (decode, audio, the device's callback, openers; player.md §1) | decodes pictures ahead of the clock and the other's frame, mixes the sound | touches the GPU's queue |
| **Viewer** (`vidcut-viewer`, `gui/video.rs`) | presents the engine: draws the picture its clock is at, answers presents and viewer screenshots | waits for the UI |
| **UI** (eframe) | shows the viewer's target as an egui texture, registers it; window screenshots | draws a viewer picture |

**Why the viewer has a thread of its own.** On GNOME Wayland with NVIDIA's driver, presenting a hidden window (minimized) blocks the UI thread until the window shows again, with Mailbox and FIFO alike (the only present modes there): measured, a seek took 2.8 s and a viewer screenshot never came. Submissions to the same device from another thread go on meanwhile, so presents, seeks and screenshots don't depend on the UI. Wayland can't show a minimized window again by request, so `gui.quit` ends the process without the window when the window hasn't closed in 5 s.

## 3. Decoding: on the GPU when there is one

- **NVDEC decodes every stream it can**, while GPU acceleration is on (the preferences' `gpu.acceleration`, the default): H.264, HEVC, AV1 and VP9 (`VideoReader::open_nvdec_or_cpu`). Streams NVDEC doesn't decode (ProRes, VP8, a profile the GPU lacks) decode on the CPU, as do stills and image sequences. Measured with `decode_speed` on this machine (16 threads, RTX 4080 SUPER):

| Stream | CPU | NVDEC |
|---|---|---|
| 2160p60 H.264 (bbb) | 1,014 frames/s; seeks into the GOP 56–255 ms | 253 frames/s; 262–874 ms |
| 2160p60 10-bit HEVC (bbb, NVENC) | 497 frames/s; 157–454 ms | 594 frames/s; 59–305 ms |
| 1080p30 H.264 (bbb) | 2,439 frames/s; 24–89 ms | 913 frames/s; 54–329 ms |

  NVDEC's engine is the limit for H.264: decoding 4K it is 99% busy (`nvidia-smi dmon`), FFmpeg's frame threads on it add nothing, and two decodes at once share its 257 frames a second. So a CPU with many cores decodes H.264 several times faster: on this machine H.264 seeks and backward windows take three to four times as long on NVDEC (§8), while playing forward takes half the CPU. HEVC, AV1 and VP9 are costly in software, and NVDEC is faster at everything. The GPU is the default all the same: it leaves the CPU to the rest of the work, and on a CPU with fewer cores it is the faster for H.264 too. Long GOPs, keyframes seconds apart as cameras, phones and the web make them (a frame late in one decodes after every frame from its keyframe), are what proxies are for (§6): a 960×400 AV1 proxy decodes at 5,921 frames a second on NVDEC, and a seek into its 60-frame GOPs takes 0–7 ms. `gpu.acceleration` off decodes everything on the CPU.
- **In video memory.** A frame NVDEC decoded stays on the GPU, NV12 or P010, and so do the pictures the engine keeps: up to a gigabyte of video memory (`Config::gpu_pictures`). FFmpeg copies each frame out of NVDEC's few surfaces into one of its own, so kept frames don't run the decoder out. A 2160p P010 picture is 24.9 MB. Frames passed over on the way to a seek are decoded and not kept.
- **Shown from there** (`gui/shared.rs`, `vidcut_av::cuda::share`): wgpu's Vulkan device makes a buffer whose memory it exports (`VK_KHR_external_memory_fd`), CUDA imports it, and to show a picture CUDA copies its planes into the buffer and wgpu copies them on into the plane textures (§4): two copies on the GPU, nothing through memory or the CPU. The whole GUI then plays 4K60 on 0.04–0.05 cores (§8). A few such buffers take turns, each free again once the queue has done the copy out of it (`Queue::on_submitted_work_done`); waiting for the GPU instead (`Device::poll`) holds wgpu's device, and the UI thread's frames stalled behind it.
- **Copied down** where the viewer can't do that: another backend than Vulkan, CUDA on another GPU than the display's, a player without a viewer (tests). The frame comes to memory at its own size with its properties and is uploaded (§4).
- **Not done: VAAPI.** The same path with VAAPI frames (AMD, Intel) would follow `open_nvdec_or_cpu`.
- **Not done: decoding into Vulkan.** Vulkan Video's images (or FFmpeg's Vulkan device made on wgpu's) would need no copy at all, and would serve AMD and Intel too (own-player.md).
- **Switching.** The session watches the preferences file; a change, from anywhere, switches the engine's decoder at once (`Player::set_hardware`), and it decodes its pictures anew, the one shown first. `player.state`'s `hwdec` says what decoded the picture shown: `nvdec` or `no`.
- **Frame-exact:** NVDEC's pictures are the CPU's, barcode for barcode, and within two levels in their pixels (`player_gpu.rs`: H.264, interlaced H.264, HEVC, VP9, AV1, HEVC in MPEG-TS and turned).

## 4. The picture on screen

- **One pass** (`gui/picture.rs`, `picture.wgsl`). A picture's planes go into textures at its own size, uploaded from memory or copied on the GPU from video memory (§3): Y, then U and V (8-bit planar) or UV interleaved (NV12, and P010 in 16-bit textures where the adapter has `TEXTURE_FORMAT_16BIT_NORM`; without them P010 goes up as its high bytes). One draw into an RGBA8 target the size of the viewer's picture in physical pixels:
  - **placed** as export fits it (`Picture::placement`), black around it;
  - **turned** upright by the display matrix (`Picture::rotation`);
  - **scaled in YUV**, as export scales before converting: going down, the average of up to 8×8 bilinear taps over the samples a pixel covers; going up, bilinear;
  - **converted** with the export's math (`vidcut_av::color::shader_colors`): the picture's matrix and range to BT.709 RGB; HDR (PQ, HLG) to linear light with the reference white at 1.0, BT.2020 to BT.709, the mobius curve on the brightest channel and BT.709's transfer as a 2.4 power: the GPU export shader's math (export.md §5), in 32-bit floats throughout.
- **Shown one to one.** egui shows the target as a native texture, nearest-sampled, in a rectangle rounded to physical pixels. A target is drawn again only when the engine's picture or the size changes.
- **Against the export** (`tests/picture.rs`): at a picture's own size, flat areas are within a level of `Picture::to_rgba` (the export's filters), PQ and HLG too; SDR stripes are within a level of the matrix's exact numbers, where swscale runs up to two or three levels low. Edges differ: at its own size the export repeats each chroma sample, the shader interpolates; scaled, swscale's bicubic is sharper than the box. Barcodes read the same at every size.
- **Present mode:** Mailbox (Immediate where there is one), so a new picture shows at the next refresh and no frames queue.

## 5. When a frame is on screen

The contract is gui.md §4's: a seek, step or viewer change returns once its frame is on screen.
- Each seek, step, load and edit's reload is a **motion**. When the engine has the picture of the frame asked for (it calls back on its decode thread), the session asks the viewer thread to present; the viewer presents the engine at that moment, draws, and answers with the frame it showed exactly. Paused, the motion is on screen when that is the frame asked for; playing, when any frame showed after it (the clock starts at the frame asked for).
- A present that showed an older picture (the frame wasn't decoded yet) is asked again; a present unanswered for 50 ms too.
- **Overtaken:** a seek whose frame another seek replaced before it showed fails with `CONFLICT`, naming the other frame (the GUI's own are no errors). Every seek is exact; `fast` (a scrub) only keeps it out of the command log.
- **Failed:** a picture that fails to decode (an unreadable file) fails the waiting seek with the reason, and `player.state` shows it.
- **Held keys:** the engine merges requests, and a paused seek finishes before the newest runs unless it has decoded for a second (player.md §2), so the picture moves at the speed frames decode and the last step lands exactly.

## 6. Proxies

A proxy is a smaller AV1 or H.264 file (8-bit 4:2:0, closed GOPs of 60 frames, every frame coded in the order it shows) of a pool item's pictures. `proxy.generate` writes one for a video or an image sequence, with the encoder the preferences' `proxies.codec` picks: hardware AV1 when one works (NVENC, VAAPI), then x264 (`auto`) or SVT-AV1 (`av1`); x264 for `h264`. Measured on 60 s of 4K60 (960 px proxies): x264 23.1 MB in 7.1 s, NVENC AV1 22.8 MB in 6.3 s at a higher SSIM, SVT-AV1 16.4 MB in 16.7 s; every one decodes at over 1,000 fps on the CPU.

**Encoding.** The request first runs on a copy of the project, which writes the files: in the GUI on a worker thread (the measure step that analysis edits use), headless before the project is locked. Then it runs for real and finds the files there, so recording them is quick. The encode reports progress through `vidcut_av::progress` and stops on the `vidcut_av::cancel` flag; a stopped encode removes its temporary file. The viewer plays it only when `settings.use_proxies` is set and the proxy is fresh: same source size and modification time as when it was made, and the file is there. Otherwise the original plays.

**Timestamps.** Each proxy frame is given the source frame's presentation timestamp (the source time base, or microseconds when that base is not a sane clock). The viewer's plan swaps only the video piece's path (`vidcut_core::preview::apply_proxies`) and keeps `origin` and `source_frame`, so the export's sampling rule picks the same frames from the proxy. An image sequence's proxy is picture `k` at `k/rate` with origin 0, and the viewer plays that video instead of the pictures.

**What does not change.** Export, frame render, analysis and OTIO keep the original paths. The viewer's sound stays on the original file. A proxy is already progressive and upright.

**On the GPU.** With NVENC, a proxy is decoded on NVDEC and scaled there (`vidcut_av::cuda`), the frames never copied to memory, when that gives the pictures the CPU would: a video file, interlaced ones deinterlaced by the reader (`bwdif_cuda`, one frame per frame, as the CPU reader). Turned pictures, other matrices (BT.601 SD, VP8), full range and HDR (tone-mapped by the CPU's curve in a shader) go through libplacebo on Vulkan first (export.md §5). Anything else, and streams NVDEC can't decode (ProRes, or a profile the GPU lacks), decode on the CPU, still encoding on NVENC.
- **The decoder:** `VideoReader::open_cuda` uses a decoder that can use NVDEC, FFmpeg's own: FFmpeg would pick libdav1d for AV1, which decodes on the CPU only.
- **Scaling:** `scale_cuda` samples a fixed few pixels per output pixel, so a big shrink aliases (4K to 960 made frames sharper than the CPU's and files 33% bigger). It halves with bilinear steps first, each the average of four pixels, then scales to the size with bicubic: as sharp as the CPU's, files 7% bigger.
- **Measured** on 176 s of 4K60 10-bit AV1 to a 960 px AV1 proxy: 107 s and 381 CPU-seconds decoding on the CPU, 37 s and 2.6 CPU-seconds on the GPU, with NVDEC 96–99% busy. Pictures match the CPU's within a level on average (10 to 8 bits is truncated, where the CPU rounds). 12 min of VP8 (BT.601) 1920×800 to 960 px: 14.4 s and 104 CPU-seconds on the CPU, 18.1 s and 7 on the GPU, NVDEC 70% busy. 30 s of 4K60 H.264: 4.2 s and 34 CPU-seconds on the CPU, 7.5 s and 0.5 on the GPU (8.1 s turned): NVDEC decodes 4K H.264 at 240 fps, the CPU's 16 threads at 430. 30 s of 4K60 PQ HEVC (SDR white at 1000 nits): 7.3 s and 66 CPU-seconds on the CPU, 4.0 s and 1.5 on the GPU (libplacebo scales it and tone-maps it in one pass).
- **Tests:** `tests/proxy.rs` (`--features gpu-tests`) makes NVENC proxies of 720p H.264, 10-bit HEVC, VP9, 10-bit AV1, an MPEG-TS starting at 1.4 s, interlaced H.264, BT.601 SD, sources turned 90° and 180°, PQ, and HLG turned 90°, each on the GPU with every frame's barcode and timestamp the source's (turned upright, tone-mapped); interlaced BT.601 stays on the GPU too, ProRes goes to the CPU. `proxy.generate` says how each proxy was made (`made_with`). `tests/live.rs` plays a proxy in the viewer frame for frame.

**Where.** Beside the project, in `proxies/`, when the session knows the project path; otherwise `~/.cache/vidcut/proxies`. The project file stores the path relative to its folder, as it does media there, so the proxies move with the project. Stills are not proxied: one picture is not the real-time problem.

## 7. Sound

- **The device.** The engine plays to the system's default output through cpal (ALSA, which PipeWire and PulseAudio serve); its callback is the clock, its latency made up for, so pictures keep with the sound as it is heard (player.md §4). When the device won't open, the engine plays to a silent stand-in and the status bar says why, once: the picture plays all the same.
- **Every lane mixed** as export mixes them: clip gains, faders, mute and solo (spec D28), de-click fades at cuts.
- **Faders while dragged** (`mixer.preview`): a gain on top of the track's lane while the viewer plays the timeline, heard within a tenth of a second; held while the source is shown, and heard again when the timeline is. Releasing saves the level with `track.set`, whose new plan has it. A track with no sound in the mix (muted, left out by solo, empty) is an error; the source shown answers `live: false`.
- **Monitoring mute** (`player.mute`) silences the engine; the clock runs on. Neither the project nor the export changes.
- **Audio scrubbing** (decision D42): steps and paused seeks play their frame's sound; `player.audio_scrubbing`, Shift+S or Timeline > Audio Scrubbing turn it off and on (on when the GUI starts). The viewer's own moves (opening at a remembered frame, after an edit) play none.
- **Backward and other speeds** (J/K/L) play the sound reversed or stretched, keeping its pitch (player.md §4).

## 8. Measured

The real GUI, a debug build of vidcut's crates (dependencies optimized), the viewer's picture about 1000×560, on this machine; CPU is every thread of the process while it plays, seeks are `vidcut player seek` until the frame shows (a CLI process each, about 15 ms of it).

| Stream | Decoded on | Played | CPU | Seeks (10 far apart) |
|---|---|---|---|---|
| 1080p30 H.264 | NVDEC / CPU | 299 frames in 10.0 s, none late or skipped | 0.18 / 0.21 cores | 18–37 ms, 470–1,350 ms far from a keyframe / 18–21 ms, 104–162 ms |
| 1080p30 HEVC | NVDEC / CPU | 179 in 6.0 s, none late | 0.04 / 0.47 cores | 17–22 ms either |
| 2160p60 H.264 | NVDEC / CPU | 598 in 10.1 s, none late or skipped | 0.05 / 0.78 cores | 514–1,466 / 104–260 ms |
| 2160p60 10-bit HEVC | NVDEC / CPU | 598 in 10.1 s, none late | 0.05 / 2.0 cores | 57–417 / 144–535 ms |
| 2160p60 10-bit AV1 | NVDEC / CPU | 598 in 10.1 s, none late | 0.04–0.05 / 2.1 cores | 69–465 / 143–553 ms |

- **NVDEC's 2160p pictures shown from video memory** (§3), 10 s of each played in the GUI's window: 0.043–0.054 cores. Copied down and uploaded in the same runs: 0.15–0.23 cores. Single runs moved by up to 0.2 cores while other work used the machine. The 1080p rows are with pictures copied down.

- **Steps back** after a seek to the middle of a GOP, `vidcut player step -1` until the frame shows, 17–25 ms of it the CLI's own round trip (3 seeks each, 20 steps after each):

| Stream | Decoded on | First step back, right after the seek / after 1.5 s | The next 19 |
|---|---|---|---|
| 1080p30 H.264 | NVDEC / CPU | 19–22 / 22–25 ms; 21–28 / 24–25 ms | median 17–18 ms, max 21–30 ms |
| 2160p60 H.264 | NVDEC / CPU | 19–25 / 24–25 ms; 22–27 / 22–24 ms | median 17–31 ms, max 21–47 ms |
| 2160p60 10-bit HEVC | NVDEC / CPU | 18–24 / 23–24 ms; 25–30 / 30–42 ms | median 17–31 ms, max 21–43 ms |
| 2160p60 10-bit AV1 | NVDEC / CPU | 18–23 / 23–24 ms; 27–30 / 29–35 ms | median 17–30 ms, max 22–40 ms |

  A seek keeps what it decodes on its way (on the CPU, its last half second; player.md §3), so the steps back after it find their pictures, right after it too. On NVDEC seeks take as long as without (4K H.264 0.5–1.2 s); on the CPU 5–110 ms longer (1080p 56–99 ms, 4K H.264 0.11–0.30 s, 4K 10-bit HEVC 0.38–0.78 s, AV1 0.51–1.0 s). With the 1 GB this used to keep, the first step back took 0.5 s (1080p on NVDEC) to 2.0 s (4K H.264 on NVDEC), 0.08–1.0 s on the CPU, after a pause too, and 4K HEVC and AV1 waited again 0.4–1.0 s a window later.
- **Picture against sound** (`tests/live.rs`, a stand-in device heard 30 ms after it pulls): the picture changed 0.4–1.4 ms after each beep was heard; every frame of 150 showed, in order. The engine's own measurements are in player.md §7.
- **Minimized** (GNOME Wayland, NVIDIA): a seek 25 ms, a viewer screenshot 23 ms.
- **Seeks after an edit of 120 cuts** wait for nothing: 6 ms to the first.

## 9. Exact frames for the two-up: decoded once either way

The Trim Editor's and Dynamic Trim's two-up show frames of a copy of the project that the player doesn't play: `vidcut_av::export::renderer::StillRenderer`, the export engine's renderer, draws them at the cell's size (`gui/stills.rs`), a thread per cell. The engine's pictures going back follow its rule.

- **The problem it solves.** Stepping back with exact seeks decodes each step from the keyframe before it; on files with keyframes seconds apart that is a few hundred frames again for every step.
- **Forward:** the file and decoder stay open, and the next frame decodes on from the one before.
- **Back:** a window renders at once, from a keyframe up to the frame asked for, and is kept. The keyframe is found with one seek and one decoded frame (where the seek lands), and the window starts at the first frame sampled at or after it, so a seek for it lands on that keyframe and nothing in the window decodes twice. A window reaches back over more keyframes to hold at least 2 s, and holds at most half the memory budget (256 MB a cell).
- **Ahead:** windows render on a second thread (`Prefetch`) before they are needed. Wherever the position is, even after a jump, at least 2 s behind it are kept; going back, a whole window more, so the next window back renders while the position goes through this one. Asked for a frame a window is rendering, the renderer waits for it instead of rendering it too.
- **Floor:** the cell says where the shown stretch of the file starts. Windows never reach behind it: frames before a clip's first one aren't shown there.
- **Eviction:** over the budget, the pictures kept or looked at least recently go first. Moving back and forth over a cut keeps both sides whole: a hole in the kept frames would decode from a keyframe while the cell waits.
- **Measured** (`tests/renderer.rs`, keyframes 120 frames apart, with B-frames): 50 steps forward decode 50 frames; 299 steps back decode 306. On a 1080p HEVC file with keyframes 8 s apart, a step back takes under 5 ms except once a window, and the first picture after a seek about 0.7 s.

## 10. Still open

- Zero-copy GPU frames, VAAPI and Vulkan Video decoding, Metal and D3D frames on other systems: own-player.md.
- Choosing the audio device: the default one plays.
- The meters show the edit's levels, not the engine's output: todo/meters-tap-player-output.md.
