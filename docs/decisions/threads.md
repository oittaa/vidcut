# Threads

Every thread of a vidcut process: what starts it and why, and how the CPU's work is shared among them. What the app's own threads own, and never do, is in [gui.md](gui.md) §1.

## 1. Rules

- **A thread waits for one thing or runs one stage.** The UI, the viewer, the session, a request, a player's pictures and its sound, a filmstrip worker, a pipeline's stage (opening the next piece, an export's encoding, measured light's measuring): each is a thread of its own, named after its work, that blocks while it has none. An idle thread costs its stack.
- **The CPU's work split into slices runs on the shared pools (§3).** Nothing starts threads for one call, one frame or one file.
- **A decoder is its reader's (§4):** opened with it, closed with it, counted while it is open.
- **Frame threads only where nothing else can split the work:** a stream decoded on the CPU (§4.2). FFmpeg runs a decoder's frame threads on threads of that decoder's own, and no outside pool can run them.
- **Jobs come last.** A job runs at the lowest priority, and its decoding waits while a player plays (gui.md §9, player.md §1). Its work on the pools runs at that priority too.
- **Named.** The app starts its threads through `threads::spawn` (gui.md §9). vidcut-av names each thread it starts `vidcut-…`. FFmpeg 8 names its frame threads itself (`av:png:df0`), so a census by name (`scripts/threads.sh`, tools.md) shows their codec, not their reader; the `[decoding]` lines show the reader.

## 2. The threads

The GUI with a project open on Linux (an NVIDIA GPU, an AMD iGPU beside it):

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
| The pools | `vidcut-cpu-N`, a thread fewer than the cores; `vidcut-cpu-job-N`, as many | the pools, on first use | §3 |
| FFmpeg | one frame thread a core for each stream decoder on the CPU (`av:…:dfN`) | the readers' decoders | §4.2 |
| | swscale's, one a core for each stream converted on the CPU | a stream's conversion | §5 |

Threads that come and go: one for each IPC connection (`vidcut-ipc-conn`) and each task (gui.md §1); a player's `vidcut-play-windows` while it follows another, `vidcut-scrub` while a viewer scrubs without one; the opener of a reader that reads pieces (`vidcut-open`) and an export's `vidcut-encode` (with its encoder's own threads); measured light's measuring threads.

So the threads a project holds are its readers': each reader's decoder closes with it, and one file at a time opens ahead (export.md §1). Playing a 4K PNG sequence holds the rows above, the player's decoder's frame threads and swscale's for its conversion to YUV (§5); the filmstrip's four workers read single frames, one thread each.

## 3. The shared pools

- **Two, each started on first use** (`pool.rs`): `vidcut-cpu-N` at normal priority, and `vidcut-cpu-job-N` at the lowest (gui.md §9's, set on each). Work runs on the pool of the thread that asks: on the jobs' for a thread working for a job (`first::as_job`), and for the threads vidcut-av starts for one (an opener, an export's encoding, measured light's measuring). So it runs at the job's priority on every system: threads on Linux would inherit it, those on Windows and macOS don't. The normal pool has a thread fewer than the cores, as the thread asking works too.
- **A call's slices,** which the calling thread works on too and waits for. The pool runs the slices of the oldest call that has some left. A slice is short: nothing waits behind more than a frame's.
- **Slices:** filter graphs'. Every graph vidcut makes, audio ones too, comes from `pool::graph`, which hands FFmpeg the app's way of running slices (`AVFilterGraph.execute`): a graph starts no thread. And the row loops of `color.rs`.
- **What can't run there:** a decoder's own slices. `AVCodecContext.execute2` gives each slice a thread number below the decoder's thread count, by which decoders pick their buffers: a decoder opened for one thread would share its one buffer among the pool's threads, and one opened for more starts threads of its own as it opens. swscale has no such hook (§5).

## 4. Decoders

Every video decoder in vidcut-av is a `VideoReader`'s (`decode.rs`): the player's readers (the one it plays with, its prepared picture's, its windows back), exports', proxies', analyses', measured light's, the filmstrip's, storyboards', `frame render`'s, thumbnails' and probes'.

### 4.1 A decoder is its reader's

- **Opened with it** (`VideoReader::open_raw`), **closed with it,** on the thread that drops the reader. Every reader counts while it is open (`decode::open_readers`), and under `VIDCUT_DEBUG` says its decoder's thread count as it opens, and how many readers are open as it opens or closes (tools.md).
- **A rewind** (a seek to the start, which some files only manage by opening again) reopens the file for the decoder it has, flushed: a decoder made anew would start a pool of frames of its own while kept pictures hold this one's.
- **Opening ahead** (`vidcut-open`, export.md §1): one file at a time, from a piece's second frame on. A source it opened that nobody wants closes on that thread.
- **A file left waits** (`VideoFrames::start_layers`, which the player, exports and the filmstrip read pieces with): a file no piece reads stays open ten seconds, two such files at most and one of them a GPU's (§4.3), the oldest closing first. A playhead dragged back across a cut seeks it rather than opening it anew, and the piece after a cutaway has it sought ahead. Files left close on the thread that reads: as a piece starts, and while a player's decode or windows thread has nothing to do.
- **A filmstrip worker's reader** closes ten seconds after its last tile, and the next tile opens one.

### 4.2 On the CPU: frame threads

- **A stream's decoder** (`Reading::Stream`, §4.4) takes FFmpeg's automatic count of frame threads, about one a core: frame threads are the only way to share one stream among the cores. They start as the decoder opens and end as it closes, and hold as many frames in flight as there are threads. libdav1d (AV1 on the CPU) starts a pool of its own from the same count.
- **Single frames** decode without them (§4.4).

### 4.3 On the GPU

A stream the GPU in use decodes, decodes there (preview.md §3): its decoder's work is the GPU's, and it opens with one thread, so FFmpeg starts none for it. An idle GPU decoder holds video memory until its reader closes.

### 4.4 Reading: streams and single frames

A reader is opened for one of two kinds of reading (`decode::Reading`):
- **A stream:** frames in order from where it starts (the player, exports, proxies, analyses, measured light). Frame threads on the CPU (§4.2), and its graphs' slices on the pool.
- **Single frames:** a frame here and there, each after a seek (filmstrip tiles, storyboards, `frame render`, thumbnails, probes; `VideoFrames::single_frames`). No frame threads: they hand a frame out only once a frame for each of them has gone in, so a frame asked for alone decoded the ones after it for nothing. Its graphs run in one slice on the reader's own thread, swscale too. The filmstrip's four workers read four tiles at once.

## 5. swscale

FFmpeg's scale filter hands swscale its graph's thread count, and swscale starts that many threads of its own: one a core in a stream's graph, none in a single frame's.

## 6. Tests

- [`crates/vidcut-av/tests/threads.rs`](../../crates/vidcut-av/tests/threads.rs) (Linux, which lists a process's threads in `/proc`): a single-frame reader starts no thread; a graph's slices start none once the pool runs; a reader counts while it is open; a file left stays open for a later piece of it, dragged back to or played across a cutaway.
- `export/video.rs`'s unit tests: a file left closes ten seconds after, past two the oldest, past one of a GPU's; `gui/tiles.rs`'s: a worker's reader closes ten seconds after its last tile.
- `export/ahead.rs`'s unit tests: a source nobody wants closes on the helper thread; a start before the one before began replaces it; a running job nobody wants stops; a dropped reader leaves its source to the helper thread.
- `player_load` ([tools.md](../tools.md)) plays a project with the real player beside filmstrip workers and a job, and seeks it around: the frames shown late, the threads at most and the peak memory.
- `scripts/threads.sh` ([tools.md](../tools.md)) counts a running GUI's threads by name once a second.
