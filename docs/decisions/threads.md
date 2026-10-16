# Threads

Every thread of a vidcut process: what starts it and why, and how the CPU's work is shared among them. What the app's own threads own, and never do, is in [gui.md](gui.md) §1.

## 1. Rules

- **A thread waits for one thing or runs one stage.** The UI, the viewer, the session, a request, a player's pictures and its sound, a filmstrip worker, a pipeline's stage (`export::ahead` opening the next piece, an export's encoding, measured light's measuring): each is a thread of its own, named after its work, that blocks while it has none. An idle thread costs its stack.
- **Work split into slices runs on the shared pools (§3),** never on threads started for one call.
- **Frame threads only where frames stream in order (§4).** FFmpeg runs a decoder's frame threads on threads of the decoder's own.
- **Jobs come last.** A job runs at the lowest priority, and its decoding waits while a player plays (gui.md §9, player.md §1). Its slices run on the pool at that priority.
- **Named.** The app starts its threads through `threads::spawn` (gui.md §9). vidcut-av names each thread it starts `vidcut-…`.

## 2. The threads

The GUI with a project open, playing a 4K PNG sequence on Linux (an NVIDIA GPU, an AMD iGPU beside it):

| Group | Threads | Started by | For |
|---|---|---|---|
| Window | the main thread (`vidcut`) | `gui::run` | the window's event loop |
| | arboard's (`vidcut`), `smithay-clipboard` | egui-winit | the clipboard on X11 and on Wayland |
| | accesskit's (`vidcut`) | accesskit_unix | screen readers (AT-SPI) |
| | `zbus::Connection`, `async-io`, `blocking-1` | zbus, for `look.rs` | the desktop's light or dark mode and accent, from its portal |
| GPU drivers | NVIDIA's 5 (`[vkcf] Analysis`, `[vkrt] Analysis`, `[vkps] Update`, two `vidcut`); RADV's 2 (`vidcut:disk$0`) | the Vulkan drivers, as wgpu lists the adapters | the drivers' own work. RADV's are the iGPU's, whose driver wgpu loads to list it. |
| | CUDA's 3 (`cuda…`, `cuda-EvtHandlr`, a third) | `cuInit` (`gpus.rs`), and the process's CUDA device (`cuda::CudaDevice::on`) | CUDA's own work |
| The app | `vidcut-viewer`, `vidcut-session`, `vidcut-ipc`, `vidcut-look`, `vidcut-missing-media`, `vidcut-worker-0` and `-1`, `vidcut-tiles-0` to `-3` | `threads::spawn` | gui.md §1 |
| | 3 `notify-rs inotify` | the `notify` crate, one for each watcher: the preferences, the themes folder, the project | changed files |
| The player | `vidcut-play-video`, `vidcut-play-audio` | `player::Player::new` | its pictures, and its sound |
| Sound out | `vidcut-audio-out`, `cpal_alsa_out`; PipeWire's 3 (`alsa-pipewire`, `data-loop.0`, `module-rt`) | `player::CpalOutput` | the device's stream: cpal's, and PipeWire's ALSA plugin |
| FFmpeg | one frame thread a core (`av:png:df0`, …) | the player's decoder | decoding (§4) |
| | swscale's, one a core | the player's conversion to YUV | §5 |

Threads that come and go: one for each IPC connection (`vidcut-ipc-conn`) and each task (gui.md §1); a player's `vidcut-play-windows` while it follows another, `vidcut-scrub` while a viewer scrubs without one; an export's `vidcut-open` and `vidcut-encode` (with its encoder's own threads); measured light's measuring threads; the shared pools' (§3) once something asks them.

## 3. The shared pools

- **Two, each started on first use** (`pool.rs`): `vidcut-cpu-N` at normal priority, and `vidcut-cpu-job-N` at the lowest (gui.md §9's, set on each). Slices run on the pool of the thread that asks: on the jobs' for a thread working for a job (`first::as_job`), and for the threads vidcut-av starts for one (`export::ahead`'s, an export's encoding, measured light's measuring). So they run at the job's priority on every system: threads on Linux would inherit it, those on Windows and macOS don't. Each pool has a thread fewer than the cores, as the thread asking works too, and runs the oldest call's slices first.
- **What runs there:** filter graphs' slices. Every graph vidcut makes, audio ones too, comes from `pool::graph`, which hands FFmpeg the app's way of running slices (`AVFilterGraph.execute`): a graph starts no thread. And the row loops of `color.rs`.
- **What can't:** a decoder's slices. `AVCodecContext.execute2` gives each slice a thread number below the decoder's thread count, by which decoders pick their buffers: a decoder opened for one thread would share its one buffer among the pool's threads, and one opened for more starts threads of its own as it opens. swscale has no such hook (§5).

## 4. Readers: streams and single frames

A reader is opened for one of two kinds of reading (`decode::Reading`):
- **A stream:** the player, exports, proxies, analyses, measured light. Its decoder takes FFmpeg's automatic count of frame threads, about one a core, and its graphs' slices run on the pool.
- **Single frames:** filmstrip tiles, storyboards, `frame render` (`VideoFrames::single_frames`, as `export::renderer` reads). Its decoder has no frame threads: they hand a frame out only once a frame for each of them has gone in, so a frame asked for alone decoded the ones after it for nothing, each held by a thread. Its graphs run in one slice on the reader's own thread, swscale too: the reader starts no thread. The filmstrip's four workers decode four tiles at once. Pool thumbnails decode with FFmpeg's default of one thread, their graphs as single frames'.
- A seek to the start opens the file again, and with it a new decoder and its frame threads (`VideoReader::rewind`).

## 5. swscale

FFmpeg's scale filter hands swscale its graph's thread count, and swscale starts that many threads of its own: one a core in a stream's graph, none in a single frame's.

## 6. Tests

- [`crates/vidcut-av/tests/threads.rs`](../../crates/vidcut-av/tests/threads.rs) (Linux, which lists a process's threads in `/proc`): a single-frame reader starts no thread, and a graph's slices start none once the pool runs.
- `player_load` ([tools.md](../tools.md)) plays a project with the real player, beside filmstrip workers and a job, and counts the frames it showed late.
