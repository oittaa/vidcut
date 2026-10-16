# Threads

Every thread of a vidcut process: what starts it and why, and how the CPU's work is shared among them. What the app's own threads own, and never do, is in [gui.md](gui.md) §1.

## 1. Rules

- **A thread waits for one thing or runs one stage.** The UI, the viewer, the session, a request, a player's pictures and its sound, a filmstrip worker, a pipeline's stage (opening the next piece, an export's encoding, measured light's measuring): each is a thread of its own, named after its work, that blocks while it has none. An idle thread costs its stack.
- **The CPU's work runs on the shared pools (§3):** slices of a picture, and the decoding of frames that stand alone (§4.2). Nothing starts threads for one call, one frame or one file.
- **Decoders belong to the decode layer (§4),** not to the readers that use them. A reader leases a decoder and gives it back. Every decoder open in the process is the layer's, counted and capped there.
- **Frame threads only where nothing else can split the work:** a long-GOP stream decoded on the CPU (§4.3). FFmpeg runs a decoder's frame threads on threads of that decoder's own, and no outside pool can run them.
- **Jobs come last.** A job runs at the lowest priority, and its decoding waits while a player plays (gui.md §9, player.md §1). Its work on the pools runs at that priority too.
- **Named.** The app starts its threads through `threads::spawn` (gui.md §9). vidcut-av names each thread it starts `vidcut-…`.

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
| The decode layer | `vidcut-close` | the layer, on its first close | closing decoders off the reader's thread (§4.1) |
| The pools | `vidcut-cpu-N`, a thread fewer than the cores; `vidcut-cpu-job-N`, as many | the pools, on first use | §3 |
| FFmpeg | one frame thread a core for each long-GOP decoder on the CPU, at most four such decoders | the layer's decoders | §4.3 |
| | swscale's, one a core for each stream converted on the CPU | a stream's conversion | §5 |

Threads that come and go: one for each IPC connection (`vidcut-ipc-conn`) and each task (gui.md §1); a player's `vidcut-play-windows` while it follows another, `vidcut-scrub` while a viewer scrubs without one; the opener of a reader that reads pieces (`vidcut-open`) and an export's `vidcut-encode` (with its encoder's own threads); measured light's measuring threads.

So the threads a project holds don't grow with what is done to it: seeks, plays, cuts, filmstrips and exports lease decoders that are there, or open them within the caps. Playing a 4K PNG sequence holds the rows above FFmpeg's, the pools, and swscale's threads for its conversion to YUV (§5): no reader holds a decoding thread of its own.

## 3. The shared pools

- **Two, each started on first use** (`pool.rs`): `vidcut-cpu-N` at normal priority, and `vidcut-cpu-job-N` at the lowest (gui.md §9's, set on each). Work runs on the pool of the thread that asks: on the jobs' for a thread working for a job (`first::as_job`), and for the threads vidcut-av starts for one (an opener, an export's encoding, measured light's measuring). So it runs at the job's priority on every system: threads on Linux would inherit it, those on Windows and macOS don't. The normal pool has a thread fewer than the cores, as the thread asking works too.
- **Two kinds of work:** a call's slices, which the calling thread works on too and waits for; and decode tasks (§4.2), which a reader queues and collects as they finish.
- **The player's first.** In the normal pool, work a player's threads ask for runs before other work (frame render, the two-up's players), and within each, the oldest first. A slice is short, a decode task one frame's decoding: nothing waits behind more than a frame.
- **Slices:** filter graphs'. Every graph vidcut makes, audio ones too, comes from `pool::graph`, which hands FFmpeg the app's way of running slices (`AVFilterGraph.execute`): a graph starts no thread. And the row loops of `color.rs`.
- **What can't run there:** a decoder's own slices. `AVCodecContext.execute2` gives each slice a thread number below the decoder's thread count, by which decoders pick their buffers: a decoder opened for one thread would share its one buffer among the pool's threads, and one opened for more starts threads of its own as it opens. swscale has no such hook (§5).

## 4. The decode layer

Every video decoder in vidcut-av comes from the decode layer (`decode::layer`): the player's readers (the one it plays with, its prepared picture's, its windows back), exports', proxies', analyses', measured light's, the filmstrip's, storyboards', `frame render`'s, thumbnails' and probes'.

### 4.1 Decoders are leased

- **The cache.** The layer keeps the decoders open in the process, each with its file open: keyed by the file (an image sequence by its pattern), the GPU decoder asked for, and how it is read (§4.5).
- **A lease.** A reader asks the layer for a decoder of its file and holds it while it reads. One reader at a time decodes from a lease. Let go, the decoder goes back to the cache with its file where it stands.
- **Asked again**, the layer hands back an idle decoder of that key, and the reader seeks with it: a frame the decoder stands just before decodes on, without a seek. Only when none is idle does it open another.
- **Idle decoders close** after ten seconds unused, and before the caps (§4.3) would be passed. They close on the layer's `vidcut-close` thread: a decoder's frame threads end and its GPU memory frees there, and the reader that let it go doesn't wait (player.md §1, Ending).
- **Opening ahead** (`vidcut-open`, export.md §1) leases the next piece's decoder. A piece that doesn't come lets it go: it is idle in the cache, for the next reader of that file or to close.
- **A rewind** (a seek to the start, which some files only manage by opening again) opens the file anew and keeps its decoder, flushed, while the stream's parameters stay the same.
- **Seen.** Under `VIDCUT_DEBUG` the layer says what it opens, hands back and closes, and lists the decoders open, leased or idle, with their readers.

### 4.2 Frames that stand alone: decoded on the pool

- **Which:** every frame of an image sequence (each file one picture), and of the codecs FFmpeg marks intra-only (`AV_CODEC_PROP_INTRA_ONLY`: ProRes, DNxHD, MJPEG, DPX, EXR, TIFF, JPEG 2000, v210, raw video and others). Not PNG in a video file, FFV1 or CineForm, whose descriptors don't say so. Decoded on the GPU where it decodes them (§4.4).
- **How:** the lease demuxes on the reader's thread and queues each packet as a decode task on the reader's pool (§3). Each pool thread keeps a decoder of its own for each kind of stream it has decoded (the stream's parameters), one thread each, which serves every file of that kind. One of a pool thread's decoders that is unused for ten seconds closes.
- **In order.** The reader takes frames in the stream's order, waiting for the next as it needs it.
- **In flight:** a stream as many frames as the pool has threads, a reader of single frames the one asked for (§4.5). A seek drops the tasks not started, and the frames of those that did.
- **What it holds:** no thread. However many readers read such files, the pool's threads decode them all, the player's first.

### 4.3 Long-GOP on the CPU: frame threads, capped

- **Which:** a stream of H.264, HEVC, AV1, VP9 and the other codecs whose frames depend on others, decoded on the CPU (a profile the GPU lacks, GPU decoding off, no GPU).
- **A stream's decoder** takes FFmpeg's automatic count of frame threads, about one a core: frame threads are the only way to share one such stream among the cores.
- **At most four** such decoders open in the process: a player's piece and its next, a job's piece and its next. An idle one closes before a fifth opens. A fifth asked for while four are leased opens without frame threads. Jobs wait while a player plays, so the frame threads at work at once are the player's.
- **Single frames** decode without frame threads (§4.5).

### 4.4 On the GPU

A stream the GPU in use decodes, decodes there (preview.md §3): its decoder's work is the GPU's, and FFmpeg starts no CPU threads for it. Its leases and idle decoders count in the cache as the CPU's do: an idle GPU decoder holds video memory until it closes.

### 4.5 Reading: streams and single frames

A lease is asked for one of two kinds of reading (`decode::Reading`):
- **A stream:** frames in order from where it starts (the player, exports, proxies, analyses, measured light). Frame threads where §4.3 has them, a pool's worth of frames in flight where §4.2 has them, and its graphs' slices on the pool.
- **Single frames:** a frame here and there, each after a seek (filmstrip tiles, storyboards, `frame render`, thumbnails, probes; `VideoFrames::single_frames`). No frame threads: they hand a frame out only once a frame for each of them has gone in, so a frame asked for alone decoded the ones after it for nothing. One task in flight where §4.2 has them. Its graphs run in one slice on the reader's own thread, swscale too. The filmstrip's four workers read four tiles at once.

## 5. swscale

FFmpeg's scale filter hands swscale its graph's thread count, and swscale starts that many threads of its own: one a core in a stream's graph, none in a single frame's.

## 6. Tests

- [`crates/vidcut-av/tests/threads.rs`](../../crates/vidcut-av/tests/threads.rs) (Linux, which lists a process's threads in `/proc`): a single-frame reader starts no thread; a graph's slices start none once the pool runs; a stream of frames that stand alone starts none once the pool runs; seeking back and forth between pieces many times holds as many threads and decoders as the first seek; a fifth long-GOP decoder on the CPU opens without frame threads.
- The layer's unit tests: an idle decoder is handed back, not opened again; one let go closes after its time, on `vidcut-close`; the caps hold.
- `player_load` ([tools.md](../tools.md)) plays a project with the real player beside filmstrip workers and a job, and seeks it around: the frames shown late, the threads at most and the peak memory.
