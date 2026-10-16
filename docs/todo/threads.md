# Threads: a bounded set that ends with its work

The process starts too many threads, and the ones it starts outlive their work. This is the plan: what runs now, the rules to get to, and the steps in order. Each step is a pull request of its own, measured with `scripts/threads.sh` before and after. [threads.md](../decisions/threads.md) is the record of what runs; this file holds what it describes but the code doesn't do yet.

## What runs now

Branch `claude/ffmpeg-threads` (PR 263). A 16-core machine; FFmpeg's automatic thread count is the cores, at most 16.

| Source | Threads | Live while |
|---|---|---|
| The app's own (gui.md §1), the window, the GPU drivers, sound out | 39 at rest | the process |
| A CPU stream decoder (`decode::Reading::Stream`, `VideoReader::open_raw`): FFmpeg's frame threads, count 0 = automatic. FFmpeg 8 names them itself (`av:png:df0` to `df15`), so the census doesn't name their owner; the `[decoding]` lines do | 16 | its reader |
| swscale, one set for each `scale` filter in a stream's graph (`pool::graph(Stream)` gives the graph the cores as `nb_threads`; the filter hands that to swscale, which starts its own, named after the thread that runs the graph) | 15 | the graph |
| libdav1d, AV1 on the CPU: a pool of its own from the same count | ~16 | its reader |
| A single-frame reader (`Reading::Frames`: filmstrip tiles, thumbnails, storyboards, frame render): one thread, swscale one | 0 | |
| A GPU decoder (`thread_count = 1`) | 0 | |
| A filter graph's own pool (slices run on `pool.rs`) | 0 | |
| The pools, `vidcut-cpu-N` and `vidcut-cpu-job-N` | 15 + 15 | the process, from first use |
| A player: `vidcut-play-video`, `-play-audio`, `-audio-out` with PipeWire's, `-play-windows` while it follows, `-scrub` | 3 to 8 | the player |
| An export: `vidcut-open`, `vidcut-encode`, the encoder's own | 2 + the encoder's | the job |
| Per request: `vidcut-task`, `-make`, `-measure`, `-ask`, `-ipc-conn`; `-two-up-close`, `-viewer-close`, `vidcut frees imports` | 1 each | the request |

**Who opens CPU stream decoders,** 31 threads each on the CPU path (one on the GPU's): the viewer's player (the current piece's layers, and the next file opened ahead), its windows reader while it follows, each two-up cell, the export job (the same, and its file ahead), proxies, analyses (`vidcut-measure`), measured light (and 2 to 4 measuring threads).

**Churn, recorded** with the census and `VIDCUT_DEBUG`: a timeline of a 4K 16-bit PNG sequence and AV1 proxies, played with the playhead dragged for two minutes. Leaving a file closes its reader and coming back opens it anew (`start_layers` keeps only the files the current piece reads): nine opens and fifteen closes of the PNG decoder on the decode thread, each open starting 16 threads and decoding a frame, some cancelled by the next seek before they were used. The export job's decoder of the PNG sequence took the automatic count, 16, on its `vidcut-open` thread, as the player's did.

**PR 263, checked against its code:** the pools, `pool::graph` for every graph, `Reading` with one thread for single frames, the opener that keeps one file ahead (export.md §1): built, and sound. Stated in threads.md but not built: §4, the decode layer (leases, the cache, `vidcut-close`, idle decoders closing, the caps, frames that stand alone decoded on the pool); §3's "the player's first" (the pool's queue is oldest-first); §2's rows for `vidcut-close` and the caps. Its numbers come from `player_load` (release build, the real player) and weren't rerun.

## Rules

1. **One set of frame threads working at a time in each class, and the class's pool.** Interactive: the normal pool and the playing player's decoder. Jobs: the jobs' pool, one job's decoder and its encoder, at the lowest priority, which takes no core from the interactive class. Frames that stand alone take no frame threads at all (step 3): the pool is their only worker set.
2. **A thread lives with its work.** A decoder closes with its reader. A reader that idles closes within 10 s, unless its next frame is expected: the player's current piece and the file ahead.
3. **Counted and seen.** Every decoder open is counted, with its owner thread and its thread count, under `VIDCUT_DEBUG` (done: `decode::open_readers`, tools.md).
4. **Nothing starts a thread for one call, one frame or one file.**

## Steps

### 1. threads.md says what runs

Cut §4, §3's "the player's first" and §2's `vidcut-close` and caps rows into this file until they are built (AGENTS.md rule 11). PR 263 can then merge: its built parts hold.

### 2. Thread counts from one budget

In `open_raw` and `pool::graph`, the count a decoder opens with and its graph's `nb_threads` come from one place, `pool::threads_for(reading, class)`:
- Interactive stream decoders (the player, the two-up, the windows reader): the cores, as now.
- Jobs' stream decoders: half the cores. A job's decoder works beside its encoder, and 16 frame threads hold 16 frames in flight (640 MB of 4K 16-bit PNG) that a job never needs. The file opened ahead takes the same count and idles after its first frame.
- A stream graph's `nb_threads`, so swscale's set, follows its decoder's count.
- Encoders: `threads` set to the cores for a job unless the preset sets it (rule 10: a `threads` the user set reaches the encoder as it is). `export/encode.rs` sets no thread count; gotchas.md says FFmpeg's default is one thread, which libx264 takes as its count. A census of an export says what they get now.
- Measured by `player_load`'s job frames a second and peak memory, and an export's wall time.

### 3. Frames that stand alone decode on the pool (threads.md §4.2 as written)

The biggest change and the biggest win for PNG sequences, ProRes, DNx, MJPEG and the other intra-only codecs: no frame threads at all. The reader demuxes on its thread and queues each packet as a task on its pool; each pool thread keeps a one-thread decoder for each kind of stream it has decoded (keyed by the stream's parameters, closed after ten seconds unused); frames are collected in the stream's order; a seek drops the tasks not started. In flight: as many frames as the pool has threads for a stream, one for a single-frame reader. Memory in flight is the same as frame threads hold, but once for the process, not once for each reader.
- Why it is right: intra-only frames don't depend on each other, so a pool of one-thread decoders gives everything frame threads give, and the pool is shared and sized to the cores.
- `pool.rs` grows a second kind of work beside slices: a task that returns a frame. The player's first in the normal pool only if a record shows a frame render or the two-up delaying it; otherwise oldest-first stays.
- Checked by `tests/threads.rs` (a stream of PNG frames starts no thread once the pool runs), `player_load`'s PNG frames a second at least what the CPU does now (about 40 fps), and `tests/player.rs`'s barcodes unchanged.

### 4. Decoders close when idle, and a file just left stays open a moment (threads.md §4.1, the part worth building)

- A registry in `decode.rs` beside the count: every `VideoReader` enters on open (file, reading, thread count, owner thread) and leaves on drop; listed under `VIDCUT_DEBUG`.
- Idle close: a tile worker drops its reader ten seconds after its last job (the queue's wait gets a timeout). Thumbnails' and storyboards' readers are per call already; the windows reader ends with following.
- The player's reader keeps the file it just left open for a few seconds, so a drag back and forth across a cut seeks instead of opening anew: the churn above. The full cache and leases of §4.1 only if a record after that still shows opens that cost.

### 5. A cap on CPU long-GOP stream decoders (threads.md §4.3)

After steps 2 to 4 the interactive decoders are bounded by the players (at most four with the windows reader) and the jobs' by the render queue, except analyses and measured light, one for each request. A cap of four job decoders with frame threads, beyond which one opens with a single thread, is a cheap backstop. Last, and only if the census shows it is needed.

## Not this work

The kept-pictures budget: memory in a long play is mostly the player's kept pictures (its budget is 30 GB).
