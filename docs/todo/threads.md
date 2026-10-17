# Threads: a bounded set that ends with its work

The process starts too many threads, and the ones it starts outlive their work. This is the plan: what runs now, the rules to get to, and the steps in order. Each step is a pull request of its own, measured with `scripts/threads.sh` before and after. [threads.md](../decisions/threads.md) is the record of what runs; this file holds what it describes but the code doesn't do yet.

## What runs now

Branch `claude/ffmpeg-threads` (PR 263). A 16-core machine; FFmpeg's automatic thread count is the cores, at most 16.

| Source | Threads | Live while |
|---|---|---|
| The app's own (gui.md §1), the window, the GPU drivers, sound out | 39 at rest | the process |
| A CPU stream decoder (`decode::Reading::Stream`, `VideoReader::open_raw`): FFmpeg's frame threads, `pool::threads_for`'s count: the cores on an interactive thread, half on a job's, at most 16. FFmpeg 8 names them itself (`av:png:df0` to `df15`), so the census doesn't name their owner; the `[decoding]` lines do | 16, or 8 | its reader |
| swscale, one set for each `scale` filter in a stream's graph (`pool::graph` gives the graph the same count as `nb_threads`; the filter hands that to swscale, which starts one fewer of its own, named after the thread that runs the graph) | 15, or 7 | the graph |
| libdav1d, AV1 on the CPU: a pool of its own from the same count | 16, or 8 | its reader |
| A single-frame reader (`Reading::Frames`: filmstrip tiles, thumbnails, storyboards, frame render): one thread, swscale one | 0 | |
| A GPU decoder (`thread_count = 1`) | 0 | |
| A filter graph's own pool (slices run on `pool.rs`) | 0 | |
| The pools, `vidcut-cpu-N` and `vidcut-cpu-job-N` | 15 + 15 | the process, from first use |
| A player: `vidcut-play-video`, `-play-audio`, `-audio-out` with PipeWire's, `-play-windows` while it follows, `-scrub` | 3 to 8 | the player |
| An export: `vidcut-open`, `vidcut-encode`, the encoder's own | 2 + the encoder's | the job |
| Per request: `vidcut-task`, `-make`, `-measure`, `-ask`, `-ipc-conn`; `-two-up-close`, `-viewer-close`, `vidcut frees imports` | 1 each | the request |

**Who opens CPU stream decoders:** the viewer's player (the current piece's layers, the next file opened ahead, and up to two files it left within ten seconds), its windows reader while it follows, each two-up cell, the export job (the same), proxies, analyses (`vidcut-measure`), measured light (and 2 to 4 measuring threads). A reader's decoder closes with it; a filmstrip worker's reader closes ten seconds after its last tile.

**PR 263, checked against its code:** the pools, `pool::graph` for every graph, `Reading` with one thread for single frames, the opener that keeps one file ahead (export.md §1), the thread budget and the files left open a moment (threads.md §4): built, and sound. Its numbers come from `player_load` (release build, the real player) and weren't rerun.

## Rules

1. **One set of frame threads working at a time in each class, and the class's pool.** Interactive: the normal pool and the playing player's decoder. Jobs: the jobs' pool, one job's decoder and its encoder, at the lowest priority, which takes no core from the interactive class. Frames that stand alone take no frame threads at all (step 1): the pool is their only worker set.
2. **A thread lives with its work.** A decoder closes with its reader, and a reader that idles closes within ten seconds unless its next frame is expected (done: threads.md §4.1).
3. **Counted and seen.** Every decoder open is counted, with its owner thread and its thread count, under `VIDCUT_DEBUG` (done: `decode::open_readers`, tools.md).
4. **Nothing starts a thread for one call, one frame or one file.**

## Steps

### 1. Frames that stand alone decode on the pool

In progress: a session's pull request against `claude/ffmpeg-threads`.

The biggest change and the biggest win for PNG sequences, ProRes, DNx, MJPEG and the other intra-only codecs: no frame threads at all. The reader demuxes on its thread and queues each packet as a task on its pool; each pool thread keeps a one-thread decoder for each kind of stream it has decoded (keyed by the stream's parameters, closed after ten seconds unused); frames are collected in the stream's order; a seek drops the tasks not started. In flight: as many frames as the pool has threads for a stream, one for a single-frame reader. Memory in flight is the same as frame threads hold, but once for the process, not once for each reader.
- Why it is right: intra-only frames don't depend on each other, so a pool of one-thread decoders gives everything frame threads give, and the pool is shared and sized to the cores.
- `pool.rs` grows a second kind of work beside slices: a task that returns a frame. The player's first in the normal pool only if a record shows a frame render or the two-up delaying it; otherwise oldest-first stays.
- Checked by `tests/threads.rs` (a stream of PNG frames starts no thread once the pool runs), `player_load`'s PNG frames a second at least what the CPU does now (about 40 fps), and `tests/player.rs`'s barcodes unchanged.

### 2. A cap on CPU long-GOP stream decoders

The interactive decoders are bounded by the players (at most four with the windows reader, and two files left a moment each) and the jobs' by the render queue, except analyses and measured light, one for each request. A cap of four job decoders with frame threads, beyond which one opens with a single thread, is a cheap backstop. Last, and only if the census shows it is needed.

### 3. A cache of decoders

A decoder leased by readers and closed when idle, handed back instead of opened anew, only if a record after steps 1 and 2 still shows opens that cost: the files left open a moment (threads.md §4.1) cover a dragged playhead.

## Not this work

The kept-pictures budget: memory in a long play is mostly the player's kept pictures (its budget is 30 GB). The export encoders that run on one thread: [encoder-threads.md](encoder-threads.md).
